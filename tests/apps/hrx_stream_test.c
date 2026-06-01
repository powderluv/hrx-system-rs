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
typedef struct hrx_executable_s *hrx_executable_t;
typedef struct hrx_buffer_ref_t { hrx_buffer_t buffer; size_t offset; size_t length; } hrx_buffer_ref_t;
typedef struct hrx_dispatch_config_t {
  uint32_t workgroup_count[3]; uint32_t workgroup_size[3]; uint32_t subgroup_size;
} hrx_dispatch_config_t;
extern hrx_status_t hrx_stream_dispatch(hrx_stream_t, hrx_executable_t, uint32_t,
                                        const hrx_dispatch_config_t *, const void *, size_t,
                                        const hrx_buffer_ref_t *, size_t, uint32_t);

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

  // NOTE: hrx_stream_advance_timeline is intentionally NOT exercised mid-stream
  // — manually advancing the timepoint without a matching queue signal makes a
  // subsequent flush wait forever on the orphaned value (the C reference hangs
  // too). It's a low-level escape hatch; we don't mix it with recorded work.
  //
  // The data-movement recording ops (fill/copy/update_buffer) are NOT exercised
  // here: on the local-task CPU backend the C reference itself rejects them
  // (PERMISSION_DENIED — allocatable host buffers lack QUEUE_TRANSFER usage
  // compatibility). They need a queue-transfer-capable buffer / GPU device, so
  // their data verification is deferred to the GPU accel path. Differential
  // behavior of the recording APIs themselves (same error on both backends) is
  // still covered by the matrix since C and Rust return identical status.

  // execution barrier records into the CB and must succeed, then flush + sync.
  s = hrx_stream_execution_barrier(st);
  check("stream_exec_barrier", s == NULL, "");
  s = hrx_stream_flush(st);
  check("stream_flush", s == NULL, "");
  s = hrx_stream_synchronize(st);
  check("stream_sync", s == NULL, "");
  s = hrx_stream_query(st, &complete);
  check("stream_query_after_work", s == NULL && complete == 1, "");

  // --- stream_dispatch: argument validation (deterministic error ladder).
  hrx_dispatch_config_t cfg = { .workgroup_count = {1,1,1}, .workgroup_size = {0,0,0}, .subgroup_size = 0 };
  hrx_status_t de = hrx_stream_dispatch(NULL, NULL, 0, &cfg, NULL, 0, NULL, 0, 0);
  check("stream_dispatch_null_stream_errors", hrx_status_code(de) == 3, "");
  hrx_status_ignore(de);
  hrx_status_t de2 = hrx_stream_dispatch(st, NULL, 0, &cfg, NULL, 0, NULL, 0, 0);
  check("stream_dispatch_null_exe_errors", hrx_status_code(de2) == 3, "");
  hrx_status_ignore(de2);

  hrx_stream_release(st);
  s = hrx_cpu_shutdown();
  check("cpu_shutdown", s == NULL, "");

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
