// Microbench of the libhrx PUBLIC C ABI (hrx_*). Links against EITHER the C
// libhrx.so.0 OR the Rust libhrx_rs.so — identical source — so the same ops are
// timed through both implementations of the same ABI. This is the
// port-regression measure: ratio = Rust / C-HRX, ~1.0 == parity.
//
// REQUIRES a GPU with fine-grained device-local memory (MI300X/gfx942); run with
// HRX_GPU_DRIVER=amdgpu. Emits the same `RESULT <cat> <name> <bytes> <median_ns>
// <p10_ns> <p90_ns> <iters>` lines as hip_bench.c so scripts/bench_compare.py
// renders it unchanged.
//
// NOTE: this is a DIFFERENT entry layer than hip_bench.c (which benches the HIP
// API via libamdhip64). Compare rust vs c-hrx here; do NOT compare these numbers
// to the CLR/HRX HIP-API table — the call paths differ.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

// --- hrx_* public ABI (subset; see libhrx/include/hrx_runtime.h) ---
typedef struct hrx_status_s *hrx_status_t;
typedef struct hrx_device_s *hrx_device_t;
typedef struct hrx_allocator_s *hrx_allocator_t;
typedef struct hrx_stream_s *hrx_stream_t;
typedef struct hrx_buffer_s *hrx_buffer_t;
typedef struct hrx_buffer_params_t {
  uint32_t type;
  uint16_t access;
  uint32_t usage;
  uint64_t queue_affinity;
} hrx_buffer_params_t;

#define HRX_MEMORY_TYPE_HOST_VISIBLE 0x00000002u
#define HRX_MEMORY_TYPE_HOST_LOCAL 0x00000046u
#define HRX_BUFFER_USAGE_DEFAULT 0x00000C03u
#define HRX_BUFFER_USAGE_MAPPING_SCOPED 0x01000000u
#define HRX_MEMORY_ACCESS_ALL 7

extern hrx_status_t hrx_gpu_initialize(uint32_t);
extern hrx_status_t hrx_gpu_shutdown(void);
extern hrx_status_t hrx_gpu_device_count(int *);
extern hrx_status_t hrx_gpu_device_get(int, hrx_device_t *);
extern int hrx_status_code(hrx_status_t);
extern void hrx_status_ignore(hrx_status_t);
extern hrx_status_t hrx_device_synchronize(hrx_device_t);
extern hrx_allocator_t hrx_device_allocator(hrx_device_t);
extern hrx_status_t hrx_allocator_allocate_buffer(hrx_allocator_t, hrx_buffer_params_t,
                                                  size_t, hrx_buffer_t *);
extern hrx_status_t hrx_buffer_allocate(hrx_stream_t, size_t, uint32_t, uint32_t, hrx_buffer_t *);
extern void hrx_buffer_release(hrx_buffer_t);
extern hrx_status_t hrx_buffer_get_size(hrx_buffer_t, size_t *);
extern hrx_status_t hrx_stream_create(hrx_device_t, uint32_t, hrx_stream_t *);
extern void hrx_stream_release(hrx_stream_t);
extern hrx_status_t hrx_stream_synchronize(hrx_stream_t);
extern hrx_status_t hrx_stream_fill_buffer(hrx_stream_t, hrx_buffer_t, size_t, size_t,
                                           const void *, size_t);
extern hrx_status_t hrx_synchronous_h2d(hrx_device_t, const void *, hrx_buffer_t, size_t, size_t);
extern hrx_status_t hrx_synchronous_d2h(hrx_device_t, hrx_buffer_t, size_t, void *, size_t);

// --- timing (identical method to hip_bench.c) ---
static uint64_t now_ns(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

static int cmp_u64(const void *a, const void *b) {
  uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
  return (x > y) - (x < y);
}

// Shared op context; ops use whichever fields they need.
typedef struct {
  hrx_device_t dev;
  hrx_allocator_t alloc;
  hrx_stream_t stream;
  hrx_buffer_t buf;
  void *host;
  size_t n;
} ctx_t;

static hrx_buffer_params_t host_params(void) {
  hrx_buffer_params_t p = {0};
  p.type = HRX_MEMORY_TYPE_HOST_LOCAL | HRX_MEMORY_TYPE_HOST_VISIBLE;
  p.access = HRX_MEMORY_ACCESS_ALL;
  p.usage = HRX_BUFFER_USAGE_DEFAULT | HRX_BUFFER_USAGE_MAPPING_SCOPED;
  p.queue_affinity = 0;
  return p;
}

// Ops set this on a non-OK status; measure() resets it before the timed loop
// and refuses to emit a RESULT row if any timed iteration failed (a failed op
// returns early and would otherwise record a misleadingly fast time).
static int g_op_fail = 0;

static void measure(const char *cat, const char *name, size_t bytes,
                    void (*fn)(void *), void *ctx, int warmup, int iters) {
  for (int i = 0; i < warmup; ++i) fn(ctx);
  uint64_t *samples = (uint64_t *)malloc((size_t)iters * sizeof(uint64_t));
  if (!samples) { fprintf(stderr, "OOM allocating samples\n"); exit(3); }
  g_op_fail = 0;
  for (int i = 0; i < iters; ++i) {
    uint64_t t0 = now_ns();
    fn(ctx);
    samples[i] = now_ns() - t0;
  }
  if (g_op_fail) {
    fprintf(stderr, "SKIP %s/%s bytes=%zu failures=%d (op errored; not reporting a misleading time)\n",
            cat, name, bytes, g_op_fail);
    free(samples);
    return;
  }
  qsort(samples, (size_t)iters, sizeof(uint64_t), cmp_u64);
  uint64_t median = samples[iters / 2];
  uint64_t p10 = samples[(int)(iters * 0.10)];
  uint64_t p90 = samples[(int)(iters * 0.90)];
  printf("RESULT %s %s %zu %llu %llu %llu %d\n", cat, name, bytes,
         (unsigned long long)median, (unsigned long long)p10,
         (unsigned long long)p90, iters);
  free(samples);
}

// --- ops ---
static void op_devsync(void *p) {
  ctx_t *c = (ctx_t *)p;
  hrx_status_t s = hrx_device_synchronize(c->dev);
  if (s) { hrx_status_ignore(s); g_op_fail++; }
}
static void op_stream_cd(void *p) {
  ctx_t *c = (ctx_t *)p;
  hrx_stream_t st = NULL;
  hrx_status_t s = hrx_stream_create(c->dev, 0, &st);
  if (s) { hrx_status_ignore(s); g_op_fail++; return; }
  hrx_stream_release(st);
}
static void op_alloc_free(void *p) {
  ctx_t *c = (ctx_t *)p;
  hrx_buffer_t b = NULL;
  hrx_buffer_params_t params = host_params();
  hrx_status_t s = hrx_allocator_allocate_buffer(c->alloc, params, c->n, &b);
  if (s) { hrx_status_ignore(s); g_op_fail++; return; }
  hrx_buffer_release(b);
}
static void op_stream_alloc_free(void *p) {
  ctx_t *c = (ctx_t *)p;
  hrx_buffer_t b = NULL;
  hrx_status_t s = hrx_buffer_allocate(c->stream, c->n,
                                       HRX_MEMORY_TYPE_HOST_LOCAL | HRX_MEMORY_TYPE_HOST_VISIBLE,
                                       HRX_BUFFER_USAGE_DEFAULT | HRX_BUFFER_USAGE_MAPPING_SCOPED, &b);
  if (s) { hrx_status_ignore(s); g_op_fail++; return; }
  hrx_buffer_release(b);
}
static void op_h2d(void *p) {
  ctx_t *c = (ctx_t *)p;
  hrx_status_t s = hrx_synchronous_h2d(c->dev, c->host, c->buf, 0, c->n);
  if (s) { hrx_status_ignore(s); g_op_fail++; }
}
static void op_d2h(void *p) {
  ctx_t *c = (ctx_t *)p;
  hrx_status_t s = hrx_synchronous_d2h(c->dev, c->buf, 0, c->host, c->n);
  if (s) { hrx_status_ignore(s); g_op_fail++; }
}
static void op_fill(void *p) {
  ctx_t *c = (ctx_t *)p;
  uint8_t pat = 0x5A;
  // fill is recorded into the stream's pending CB; synchronize to measure
  // completion (hrx_device_synchronize is a deprecated no-op, do NOT use it).
  hrx_status_t s = hrx_stream_fill_buffer(c->stream, c->buf, 0, c->n, &pat, 1);
  if (s) {
    // drain the stream so a half-recorded CB doesn't bleed into the next iter.
    hrx_status_ignore(s);
    hrx_status_ignore(hrx_stream_synchronize(c->stream));
    g_op_fail++;
    return;
  }
  s = hrx_stream_synchronize(c->stream);
  if (s) { hrx_status_ignore(s); g_op_fail++; }
}

// Mandatory gate: H2D->D2H byte-exact roundtrip through the public ABI.
static int correctness_gate(hrx_device_t dev, hrx_allocator_t alloc) {
  const size_t n = 1 << 16;
  hrx_buffer_t b = NULL;
  hrx_buffer_params_t params = host_params();
  hrx_status_t s = hrx_allocator_allocate_buffer(alloc, params, n, &b);
  if (s || !b) { if (s) hrx_status_ignore(s); return 0; }
  unsigned char *host = (unsigned char *)malloc(n);
  unsigned char *back = (unsigned char *)malloc(n);
  for (size_t i = 0; i < n; ++i) host[i] = (unsigned char)(i * 7 + 3);
  memset(back, 0xAB, n);
  int ok = 1;
  s = hrx_synchronous_h2d(dev, host, b, 0, n); if (s) { hrx_status_ignore(s); ok = 0; }
  s = hrx_synchronous_d2h(dev, b, 0, back, n); if (s) { hrx_status_ignore(s); ok = 0; }
  if (ok) ok = (memcmp(host, back, n) == 0);
  free(host); free(back); hrx_buffer_release(b);
  return ok;
}

// Optional: does stream fill complete + read back correctly on this backend?
// (On CPU local-task fill is rejected; on amdgpu it may work. Both libs use the
// same IREE HAL so the answer is identical for c-hrx and rust.)
static int fill_probe(hrx_device_t dev, hrx_allocator_t alloc, hrx_stream_t stream) {
  const size_t n = 1 << 16;
  hrx_buffer_t b = NULL;
  hrx_buffer_params_t params = host_params();
  hrx_status_t s = hrx_allocator_allocate_buffer(alloc, params, n, &b);
  if (s || !b) { if (s) hrx_status_ignore(s); return 0; }
  uint8_t pat = 0x5A;
  int ok = 1;
  s = hrx_stream_fill_buffer(stream, b, 0, n, &pat, 1); if (s) { hrx_status_ignore(s); ok = 0; }
  if (ok) { s = hrx_stream_synchronize(stream); if (s) { hrx_status_ignore(s); ok = 0; } }
  if (ok) {
    unsigned char *back = (unsigned char *)malloc(n);
    s = hrx_synchronous_d2h(dev, b, 0, back, n); if (s) { hrx_status_ignore(s); ok = 0; }
    for (size_t i = 0; ok && i < n; ++i) if (back[i] != 0x5A) ok = 0;
    free(back);
  }
  hrx_buffer_release(b);
  return ok;
}

int main(void) {
  hrx_status_t s = hrx_gpu_initialize(0);
  if (s) {
    fprintf(stderr, "hrx_gpu_initialize failed code=%d\n", hrx_status_code(s));
    hrx_status_ignore(s);
    printf("GATE FAILED\n");
    return 2;
  }
  int count = 0;
  s = hrx_gpu_device_count(&count);
  if (s) hrx_status_ignore(s);
  fprintf(stderr, "devices=%d\n", count);
  if (count <= 0) { fprintf(stderr, "no device\n"); return 1; }
  hrx_device_t dev = NULL;
  s = hrx_gpu_device_get(0, &dev);
  if (s || !dev) { fprintf(stderr, "device_get failed\n"); if (s) hrx_status_ignore(s); return 1; }
  hrx_allocator_t alloc = hrx_device_allocator(dev);

  if (!correctness_gate(dev, alloc)) {
    fprintf(stderr, "CORRECTNESS GATE FAILED — refusing to report numbers\n");
    printf("GATE FAILED\n");
    return 2;
  }
  printf("GATE OK\n");
  fflush(stdout);

  hrx_stream_t stream = NULL;
  s = hrx_stream_create(dev, 0, &stream);
  if (s || !stream) { fprintf(stderr, "stream_create failed\n"); if (s) hrx_status_ignore(s); return 1; }
  int fill_ok = fill_probe(dev, alloc, stream);
  fprintf(stderr, "fill_supported=%d\n", fill_ok);

  ctx_t c = {0};
  c.dev = dev; c.alloc = alloc; c.stream = stream;

  // host-API call overhead
  measure("hostapi", "deviceSynchronize", 0, op_devsync, &c, 100, 5000);
  measure("hostapi", "streamCreateDestroy", 0, op_stream_cd, &c, 50, 2000);

  // plain (non-queue-ordered) allocator alloc+free
  size_t alloc_sizes[] = {4096, 1u << 20, 64u << 20};
  for (size_t i = 0; i < sizeof(alloc_sizes) / sizeof(*alloc_sizes); ++i) {
    c.n = alloc_sizes[i];
    measure("alloc", "allocFree", c.n, op_alloc_free, &c, 20, 1000);
  }
  // stream-ordered alloc (exercises the hrx exact pool + queue_alloca + sem wait)
  size_t salloc_sizes[] = {4096, 1u << 20};
  for (size_t i = 0; i < sizeof(salloc_sizes) / sizeof(*salloc_sizes); ++i) {
    c.n = salloc_sizes[i];
    measure("alloc", "streamAllocFree", c.n, op_stream_alloc_free, &c, 5, 200);
  }

  // transfers + fill, per size (allocate one buffer of each size)
  size_t xfer_sizes[] = {4096, 64u << 10, 1u << 20, 16u << 20, 256u << 20};
  size_t max_n = 256u << 20;
  void *host = malloc(max_n);
  if (!host) { fprintf(stderr, "OOM allocating %zu host buffer\n", max_n); return 3; }
  memset(host, 0x3C, max_n);
  for (size_t i = 0; i < sizeof(xfer_sizes) / sizeof(*xfer_sizes); ++i) {
    size_t n = xfer_sizes[i];
    hrx_buffer_t b = NULL;
    hrx_buffer_params_t params = host_params();
    hrx_status_t bs = hrx_allocator_allocate_buffer(alloc, params, n, &b);
    if (bs || !b) { if (bs) hrx_status_ignore(bs); fprintf(stderr, "alloc %zu failed\n", n); continue; }
    c.buf = b; c.host = host; c.n = n;
    int iters = n >= (16u << 20) ? 200 : 2000;
    measure("xfer", "H2D", n, op_h2d, &c, 10, iters);
    measure("xfer", "D2H", n, op_d2h, &c, 10, iters);
    if (fill_ok) measure("xfer", "fill", n, op_fill, &c, 10, iters);
    hrx_buffer_release(b);
  }
  free(host);

  hrx_stream_release(stream);
  s = hrx_gpu_shutdown();
  if (s) hrx_status_ignore(s);
  printf("DONE\n");
  return 0;
}
