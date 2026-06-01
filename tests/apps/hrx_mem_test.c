// Differential test for the libhrx memory path (allocator, buffer, transfer).
// Links against EITHER the C libhrx.so OR the Rust libhrx_rs.so; identical
// output. Uses the CPU (local-task) accelerator. Emits `CHECK <name> <PASS|FAIL>`.
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct hrx_status_s *hrx_status_t;
typedef struct hrx_device_s *hrx_device_t;
typedef struct hrx_allocator_s *hrx_allocator_t;
typedef struct hrx_buffer_s *hrx_buffer_t;

typedef struct hrx_buffer_params_t {
  uint32_t type;
  uint16_t access;
  uint32_t usage;
  uint64_t queue_affinity;
} hrx_buffer_params_t;

// memory type / usage / access constants
#define HRX_MEMORY_TYPE_HOST_VISIBLE 0x00000002u
#define HRX_MEMORY_TYPE_HOST_LOCAL 0x00000046u
#define HRX_BUFFER_USAGE_TRANSFER 0x00000003u
#define HRX_BUFFER_USAGE_MAPPING_SCOPED 0x01000000u
#define HRX_BUFFER_USAGE_DEFAULT 0x00000C03u
#define HRX_MEMORY_ACCESS_ALL 7
#define HRX_MAP_READ 1
#define HRX_MAP_WRITE 2

extern hrx_status_t hrx_cpu_initialize(uint32_t);
extern hrx_status_t hrx_cpu_shutdown(void);
extern hrx_status_t hrx_cpu_device_get(int, hrx_device_t *);
extern int hrx_status_code(hrx_status_t);
extern void hrx_status_ignore(hrx_status_t);

extern hrx_allocator_t hrx_device_allocator(hrx_device_t);
extern hrx_status_t hrx_allocator_allocate_buffer(hrx_allocator_t, hrx_buffer_params_t,
                                                  size_t, hrx_buffer_t *);
extern hrx_status_t hrx_buffer_get_size(hrx_buffer_t, size_t *);
extern hrx_status_t hrx_buffer_map(hrx_buffer_t, uint32_t flags, size_t off,
                                   size_t size, void **ptr);
extern hrx_status_t hrx_buffer_unmap(hrx_buffer_t);
extern void hrx_buffer_retain(hrx_buffer_t);
extern void hrx_buffer_release(hrx_buffer_t);
extern hrx_status_t hrx_synchronous_h2d(hrx_device_t, const void *src, hrx_buffer_t dst,
                                        size_t dst_off, size_t size);
extern hrx_status_t hrx_synchronous_d2h(hrx_device_t, hrx_buffer_t src, size_t src_off,
                                        void *dst, size_t size);

static int g_fail = 0;
static void check(const char *name, int pass, const char *detail) {
  printf("CHECK %s %s %s\n", name, pass ? "PASS" : "FAIL", detail ? detail : "");
  if (!pass) g_fail++;
}

int main(void) {
  char d[128];
  hrx_status_t s = hrx_cpu_initialize(0);
  check("cpu_init", s == NULL, "");
  hrx_device_t dev = NULL;
  hrx_cpu_device_get(0, &dev);

  hrx_allocator_t alloc = hrx_device_allocator(dev);
  check("device_allocator_nonnull", alloc != NULL, "");

  const size_t N = 4096;
  hrx_buffer_params_t params = {
      .type = HRX_MEMORY_TYPE_HOST_LOCAL | HRX_MEMORY_TYPE_HOST_VISIBLE,
      .access = HRX_MEMORY_ACCESS_ALL,
      .usage = HRX_BUFFER_USAGE_DEFAULT | HRX_BUFFER_USAGE_MAPPING_SCOPED,
      .queue_affinity = 0,
  };
  hrx_buffer_t buf = NULL;
  s = hrx_allocator_allocate_buffer(alloc, params, N, &buf);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(s));
  check("allocate_buffer", s == NULL && buf != NULL, d);
  if (!buf) { printf("SUMMARY FAIL failures=%d\n", ++g_fail); return 1; }

  size_t sz = 0;
  s = hrx_buffer_get_size(buf, &sz);
  snprintf(d, sizeof d, "size=%zu", sz);
  check("buffer_get_size", s == NULL && sz == N, d);

  // H2D, then D2H roundtrip via the synchronous transfer API.
  unsigned char host_in[4096], host_out[4096];
  for (size_t i = 0; i < N; ++i) host_in[i] = (unsigned char)(i * 31 + 7);
  s = hrx_synchronous_h2d(dev, host_in, buf, 0, N);
  check("h2d", s == NULL, "");
  memset(host_out, 0xEE, N);
  s = hrx_synchronous_d2h(dev, buf, 0, host_out, N);
  check("d2h", s == NULL, "");
  check("h2d_d2h_roundtrip", memcmp(host_in, host_out, N) == 0, "");

  // out-of-range transfer must error (OUT_OF_RANGE=11)
  hrx_status_t oor = hrx_synchronous_h2d(dev, host_in, buf, N - 10, 100);
  check("h2d_oob_errors", hrx_status_code(oor) == 11, "");
  hrx_status_ignore(oor);

  // map, read back the bytes we wrote, then write via map and confirm D2H sees it
  void *mapped = NULL;
  s = hrx_buffer_map(buf, HRX_MAP_READ | HRX_MAP_WRITE, 0, N, &mapped);
  check("buffer_map", s == NULL && mapped != NULL, "");
  if (mapped) {
    check("map_sees_h2d_data", memcmp(mapped, host_in, N) == 0, "");
    // double map must fail (FAILED_PRECONDITION=9)
    void *m2 = NULL;
    hrx_status_t dm = hrx_buffer_map(buf, HRX_MAP_READ, 0, N, &m2);
    check("double_map_errors", hrx_status_code(dm) == 9, "");
    hrx_status_ignore(dm);
    s = hrx_buffer_unmap(buf);
    check("buffer_unmap", s == NULL, "");
    // unmap again is a no-op
    s = hrx_buffer_unmap(buf);
    check("buffer_unmap_noop", s == NULL, "");
  }

  hrx_buffer_retain(buf);
  hrx_buffer_release(buf); // back to 1
  hrx_buffer_release(buf); // frees
  check("buffer_retain_release", 1, "");

  s = hrx_cpu_shutdown();
  check("cpu_shutdown", s == NULL, "");

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
