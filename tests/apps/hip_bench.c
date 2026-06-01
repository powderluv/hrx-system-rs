// Microbenchmark for the HIP host-API + memory-transfer paths, used to compare
// backends (vanilla CLR vs HRX vs — later — the Rust port) on identical code.
//
// Self-declares the HIP entry points it uses (no ROCm headers needed); the
// backend is whatever provides libamdhip64 at link/load time. Reports median
// (and p10/p90) over many iterations after warmup, plus a correctness gate so
// we never report numbers for a backend that computes the wrong answer.
//
// Covers:
//   - host API latency: hipSetDevice, hipDeviceSynchronize, hipStreamCreate/Destroy
//   - alloc/free latency: hipMalloc + hipFree at several sizes
//   - transfer bandwidth: H2D and D2H memcpy across sizes (+ trailing sync)
//   - memset bandwidth (+ trailing sync)
//
// Transfer/memset timings include a trailing hipDeviceSynchronize() so both a
// sync backend (HRX) and an async one (CLR hipMemset) measure time-to-
// completion — otherwise the comparison is submit-vs-completion and meaningless.
//
// Output is machine-parseable: `RESULT <category> <name> <bytes> <median_ns> <p10_ns> <p90_ns> <iters>`
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef int hipError_t;
typedef void *hipStream_t;
extern hipError_t hipGetDeviceCount(int *);
extern hipError_t hipSetDevice(int);
extern hipError_t hipDeviceSynchronize(void);
extern hipError_t hipMalloc(void **, size_t);
extern hipError_t hipFree(void *);
extern hipError_t hipHostMalloc(void **, size_t, unsigned int);
extern hipError_t hipHostFree(void *);
extern hipError_t hipMemcpy(void *, const void *, size_t, int);
extern hipError_t hipMemset(void *, int, size_t);
extern hipError_t hipStreamCreate(hipStream_t *);
extern hipError_t hipStreamDestroy(hipStream_t);

#define H2D 1
#define D2H 2

static inline uint64_t now_ns(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}

static int cmp_u64(const void *a, const void *b) {
  uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
  return (x > y) - (x < y);
}

// Run `fn` `iters` times (after `warmup`), report percentiles.
typedef void (*bench_fn)(void *ctx);
static void measure(const char *cat, const char *name, size_t bytes,
                    bench_fn fn, void *ctx, int warmup, int iters) {
  for (int i = 0; i < warmup; ++i) fn(ctx);
  uint64_t *samples = malloc((size_t)iters * sizeof(uint64_t));
  for (int i = 0; i < iters; ++i) {
    uint64_t t0 = now_ns();
    fn(ctx);
    samples[i] = now_ns() - t0;
  }
  qsort(samples, iters, sizeof(uint64_t), cmp_u64);
  uint64_t med = samples[iters / 2];
  uint64_t p10 = samples[(int)(iters * 0.10)];
  uint64_t p90 = samples[(int)(iters * 0.90)];
  printf("RESULT %s %s %zu %llu %llu %llu %d\n", cat, name, bytes,
         (unsigned long long)med, (unsigned long long)p10,
         (unsigned long long)p90, iters);
  fflush(stdout);
  free(samples);
}

// --- workloads ---
static void w_sync(void *c) { (void)c; hipDeviceSynchronize(); }
static void w_setdev(void *c) { (void)c; hipSetDevice(0); }

struct stream_ctx { hipStream_t s; };
static void w_stream_cycle(void *c) {
  struct stream_ctx *x = c;
  hipStreamCreate(&x->s);
  hipStreamDestroy(x->s);
}

struct alloc_ctx { size_t n; };
static void w_malloc_free(void *c) {
  struct alloc_ctx *x = c;
  void *p = NULL;
  hipMalloc(&p, x->n);
  hipFree(p);
}

struct xfer_ctx { void *dev; void *host; size_t n; int kind; };
// Each timed op is followed by hipDeviceSynchronize() so BOTH backends measure
// time-to-completion, not just submit. (CLR's hipMemset is async; HRX's is
// internally synchronous — without this sync the two would not be comparable.)
static void w_memcpy(void *c) {
  struct xfer_ctx *x = c;
  if (x->kind == H2D) hipMemcpy(x->dev, x->host, x->n, H2D);
  else hipMemcpy(x->host, x->dev, x->n, D2H);
  hipDeviceSynchronize();
}
static void w_memset(void *c) {
  struct xfer_ctx *x = c;
  hipMemset(x->dev, 0x5A, x->n);
  hipDeviceSynchronize();
}

// Correctness gate: H2D + D2H roundtrip and memset must be byte-exact, else we
// bail without printing RESULT lines (so a broken backend yields no numbers).
static int correctness_gate(void) {
  const size_t n = 1 << 16;
  void *d = NULL;
  if (hipMalloc(&d, n) != 0 || !d) return 0;
  char *host = malloc(n), *back = malloc(n);
  for (size_t i = 0; i < n; ++i) host[i] = (char)(i * 7 + 3);
  hipMemcpy(d, host, n, H2D);
  memset(back, 0xAB, n);
  hipMemcpy(back, d, n, D2H);
  int ok = memcmp(host, back, n) == 0;
  hipMemset(d, 0x5A, n);
  hipMemcpy(back, d, n, D2H);
  for (size_t i = 0; i < n && ok; ++i)
    if ((unsigned char)back[i] != 0x5A) ok = 0;
  hipFree(d);
  free(host); free(back);
  return ok;
}

int main(void) {
  int count = 0;
  hipGetDeviceCount(&count);
  if (count <= 0) { fprintf(stderr, "no device\n"); return 1; }
  hipSetDevice(0);
  fprintf(stderr, "devices=%d\n", count);

  if (!correctness_gate()) {
    fprintf(stderr, "CORRECTNESS GATE FAILED — refusing to report numbers\n");
    printf("GATE FAILED\n");
    return 2;
  }
  printf("GATE OK\n");
  fflush(stdout);

  // Host API latencies.
  measure("hostapi", "deviceSynchronize", 0, w_sync, NULL, 100, 5000);
  measure("hostapi", "setDevice", 0, w_setdev, NULL, 100, 5000);
  struct stream_ctx sc;
  measure("hostapi", "streamCreateDestroy", 0, w_stream_cycle, &sc, 50, 2000);

  // Alloc/free at sizes.
  size_t alloc_sizes[] = {4096, 1 << 20, 64u << 20};
  for (size_t i = 0; i < sizeof(alloc_sizes) / sizeof(*alloc_sizes); ++i) {
    struct alloc_ctx ac = {alloc_sizes[i]};
    measure("alloc", "mallocFree", alloc_sizes[i], w_malloc_free, &ac, 20, 1000);
  }

  // Transfers + memset at sizes.
  size_t xfer_sizes[] = {4096, 64u << 10, 1u << 20, 16u << 20, 256u << 20};
  for (size_t i = 0; i < sizeof(xfer_sizes) / sizeof(*xfer_sizes); ++i) {
    size_t n = xfer_sizes[i];
    void *dev = NULL;
    if (hipMalloc(&dev, n) != 0) continue;
    void *host = NULL;
    if (hipHostMalloc(&host, n, 0) != 0 || !host) { host = malloc(n); }
    memset(host, 0x33, n);
    int iters = n >= (16u << 20) ? 200 : 2000;
    struct xfer_ctx h2d = {dev, host, n, H2D};
    struct xfer_ctx d2h = {dev, host, n, D2H};
    struct xfer_ctx ms = {dev, host, n, 0};
    measure("xfer", "H2D", n, w_memcpy, &h2d, 10, iters);
    measure("xfer", "D2H", n, w_memcpy, &d2h, 10, iters);
    measure("xfer", "memset", n, w_memset, &ms, 10, iters);
    hipFree(dev);
  }

  printf("DONE\n");
  return 0;
}
