// Differential test for the libhrx semaphore + stream path. Links against the C
// libhrx.so or the Rust libhrx_rs.so; identical output. CPU (local-task) accel.
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct hrx_status_s *hrx_status_t;
typedef struct hrx_device_s *hrx_device_t;
typedef struct hrx_allocator_s *hrx_allocator_t;
typedef struct hrx_buffer_s *hrx_buffer_t;
typedef struct hrx_semaphore_s *hrx_semaphore_t;
typedef struct hrx_stream_s *hrx_stream_t;
typedef struct hrx_timeline_point_t { hrx_semaphore_t semaphore; uint64_t value; } hrx_timeline_point_t;
typedef struct hrx_buffer_params_t {
  uint32_t type; uint16_t access; uint32_t usage; uint64_t queue_affinity;
} hrx_buffer_params_t;

#define HRX_MEMORY_TYPE_HOST_VISIBLE 0x00000002u
#define HRX_MEMORY_TYPE_HOST_LOCAL 0x00000046u
#define HRX_BUFFER_USAGE_DEFAULT 0x00000C03u
#define HRX_BUFFER_USAGE_MAPPING_SCOPED 0x01000000u
#define HRX_MEMORY_ACCESS_ALL 7

extern hrx_status_t hrx_cpu_initialize(uint32_t);
extern hrx_status_t hrx_cpu_shutdown(void);
extern hrx_status_t hrx_cpu_device_get(int, hrx_device_t *);
extern int hrx_status_code(hrx_status_t);
extern void hrx_status_ignore(hrx_status_t);
extern hrx_allocator_t hrx_device_allocator(hrx_device_t);
extern hrx_status_t hrx_allocator_allocate_buffer(hrx_allocator_t, hrx_buffer_params_t, size_t, hrx_buffer_t *);
extern void hrx_buffer_release(hrx_buffer_t);
extern hrx_status_t hrx_synchronous_d2h(hrx_device_t, hrx_buffer_t, size_t, void *, size_t);
extern hrx_status_t hrx_synchronous_h2d(hrx_device_t, const void *, hrx_buffer_t, size_t, size_t);

extern hrx_status_t hrx_semaphore_create(hrx_device_t, uint64_t, hrx_semaphore_t *);
extern void hrx_semaphore_release(hrx_semaphore_t);
extern hrx_status_t hrx_semaphore_query(hrx_semaphore_t, uint64_t *);
extern hrx_status_t hrx_semaphore_signal(hrx_semaphore_t, uint64_t);
extern hrx_status_t hrx_semaphore_wait(hrx_semaphore_t, uint64_t, uint64_t);

extern hrx_status_t hrx_stream_create(hrx_device_t, uint32_t, hrx_stream_t *);
extern void hrx_stream_release(hrx_stream_t);
extern hrx_status_t hrx_stream_get_device(hrx_stream_t, hrx_device_t *);
extern hrx_status_t hrx_stream_get_semaphore(hrx_stream_t, hrx_semaphore_t *);
extern hrx_status_t hrx_stream_get_timeline_position(hrx_stream_t, hrx_timeline_point_t *);
extern hrx_status_t hrx_stream_advance_timeline(hrx_stream_t, uint64_t *);
extern hrx_status_t hrx_stream_query(hrx_stream_t, int *);
extern hrx_status_t hrx_stream_flush(hrx_stream_t);
extern hrx_status_t hrx_stream_synchronize(hrx_stream_t);
extern hrx_status_t hrx_stream_fill_buffer(hrx_stream_t, hrx_buffer_t, size_t, size_t, const void *, size_t);
extern hrx_status_t hrx_stream_copy_buffer(hrx_stream_t, hrx_buffer_t, size_t, hrx_buffer_t, size_t, size_t);
extern hrx_status_t hrx_stream_update_buffer(hrx_stream_t, const void *, size_t, hrx_buffer_t, size_t);
extern hrx_status_t hrx_stream_execution_barrier(hrx_stream_t);

static int g_fail = 0;
static void check(const char *n, int pass, const char *d) {
  printf("CHECK %s %s %s\n", n, pass ? "PASS" : "FAIL", d ? d : "");
  if (!pass) g_fail++;
}

static hrx_buffer_t mkbuf(hrx_allocator_t a, size_t n) {
  hrx_buffer_params_t p = {
      .type = HRX_MEMORY_TYPE_HOST_LOCAL | HRX_MEMORY_TYPE_HOST_VISIBLE,
      .access = HRX_MEMORY_ACCESS_ALL,
      .usage = HRX_BUFFER_USAGE_DEFAULT | HRX_BUFFER_USAGE_MAPPING_SCOPED,
      .queue_affinity = 0};
  hrx_buffer_t b = NULL;
  hrx_allocator_allocate_buffer(a, p, n, &b);
  return b;
}

int main(void) {
  char d[128];
  hrx_cpu_initialize(0);
  hrx_device_t dev = NULL; hrx_cpu_device_get(0, &dev);
  hrx_allocator_t alloc = hrx_device_allocator(dev);

  // --- semaphore ---
  hrx_semaphore_t sem = NULL;
  hrx_status_t s = hrx_semaphore_create(dev, 5, &sem);
  check("sem_create", s == NULL && sem != NULL, "");
  uint64_t v = 0;
  s = hrx_semaphore_query(sem, &v);
  snprintf(d, sizeof d, "v=%llu", (unsigned long long)v);
  check("sem_query_initial", s == NULL && v == 5, d);
  s = hrx_semaphore_signal(sem, 10);
  check("sem_signal", s == NULL, "");
  s = hrx_semaphore_wait(sem, 10, UINT64_MAX);
  check("sem_wait", s == NULL, "");
  s = hrx_semaphore_query(sem, &v);
  snprintf(d, sizeof d, "v=%llu", (unsigned long long)v);
  check("sem_query_after_signal", s == NULL && v == 10, d);
  // wait with 0 timeout on an unmet value should NOT be OK
  hrx_status_t w = hrx_semaphore_wait(sem, 999, 0);
  check("sem_wait_timeout_unmet_errors", w != NULL, "");
  hrx_status_ignore(w);
  hrx_semaphore_release(sem);

  // --- stream lifecycle / accessors ---
  hrx_stream_t st = NULL;
  s = hrx_stream_create(dev, 0, &st);
  check("stream_create", s == NULL && st != NULL, "");
  hrx_device_t gd = NULL;
  s = hrx_stream_get_device(st, &gd);
  check("stream_get_device", s == NULL && gd == dev, "");
  hrx_semaphore_t gs = NULL;
  s = hrx_stream_get_semaphore(st, &gs);
  check("stream_get_semaphore", s == NULL && gs != NULL, "");
  hrx_timeline_point_t pt;
  s = hrx_stream_get_timeline_position(st, &pt);
  snprintf(d, sizeof d, "val=%llu", (unsigned long long)pt.value);
  check("stream_timeline_initial", s == NULL && pt.value == 0, d);
  int complete = 0;
  s = hrx_stream_query(st, &complete);
  check("stream_query_empty_complete", s == NULL && complete == 1, "");
  uint64_t adv = 0;
  s = hrx_stream_advance_timeline(st, &adv);
  snprintf(d, sizeof d, "adv=%llu", (unsigned long long)adv);
  check("stream_advance", s == NULL && adv == 1, d);

  // --- stream recording: fill then read back ---
  const size_t N = 256;
  hrx_buffer_t buf = mkbuf(alloc, N);
  check("buf_alloc", buf != NULL, "");
  uint32_t pattern = 0xABCD1234u;
  s = hrx_stream_fill_buffer(st, buf, 0, N, &pattern, sizeof pattern);
  check("stream_fill", s == NULL, "");
  s = hrx_stream_synchronize(st);
  check("stream_sync_after_fill", s == NULL, "");
  uint32_t out[64];
  memset(out, 0, sizeof out);
  s = hrx_synchronous_d2h(dev, buf, 0, out, N);
  check("d2h_after_fill", s == NULL, "");
  int fill_ok = 1;
  for (size_t i = 0; i < N / sizeof(uint32_t); ++i) if (out[i] != pattern) fill_ok = 0;
  check("fill_pattern_correct", fill_ok, "");

  // --- stream copy: src -> dst ---
  hrx_buffer_t dst = mkbuf(alloc, N);
  s = hrx_stream_copy_buffer(st, buf, 0, dst, 0, N);
  check("stream_copy", s == NULL, "");
  s = hrx_stream_synchronize(st);
  check("stream_sync_after_copy", s == NULL, "");
  memset(out, 0, sizeof out);
  hrx_synchronous_d2h(dev, dst, 0, out, N);
  int copy_ok = 1;
  for (size_t i = 0; i < N / sizeof(uint32_t); ++i) if (out[i] != pattern) copy_ok = 0;
  check("copy_correct", copy_ok, "");

  // --- stream update_buffer (host -> buffer) ---
  uint8_t hostdata[256];
  for (size_t i = 0; i < N; ++i) hostdata[i] = (uint8_t)(i ^ 0x5A);
  s = hrx_stream_update_buffer(st, hostdata, N, dst, 0);
  check("stream_update", s == NULL, "");
  s = hrx_stream_synchronize(st);
  check("stream_sync_after_update", s == NULL, "");
  uint8_t obytes[256];
  memset(obytes, 0, sizeof obytes);
  hrx_synchronous_d2h(dev, dst, 0, obytes, N);
  check("update_correct", memcmp(obytes, hostdata, N) == 0, "");

  // execution barrier (no data effect, just must succeed + flush)
  s = hrx_stream_execution_barrier(st);
  check("stream_exec_barrier", s == NULL, "");
  s = hrx_stream_flush(st);
  check("stream_flush", s == NULL, "");

  hrx_buffer_release(buf);
  hrx_buffer_release(dst);
  hrx_stream_release(st);
  s = hrx_cpu_shutdown();
  check("cpu_shutdown", s == NULL, "");

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
