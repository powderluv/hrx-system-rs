// Differential test for libhrx direct queue ops. Links against C libhrx.so or
// Rust libhrx_rs.so; identical output. CPU (local-task) accel.
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct hrx_status_s *hrx_status_t;
typedef struct hrx_device_s *hrx_device_t;
typedef struct hrx_allocator_s *hrx_allocator_t;
typedef struct hrx_buffer_s *hrx_buffer_t;
typedef struct hrx_semaphore_s *hrx_semaphore_t;
typedef struct hrx_semaphore_list_t {
  hrx_semaphore_t *semaphores;
  uint64_t *values;
  size_t count;
} hrx_semaphore_list_t;
typedef struct hrx_buffer_params_t {
  uint32_t type; uint16_t access; uint32_t usage; uint64_t queue_affinity;
} hrx_buffer_params_t;
typedef hrx_status_t (*hrx_host_call_fn_t)(void *user_data);

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
extern hrx_status_t hrx_semaphore_create(hrx_device_t, uint64_t, hrx_semaphore_t *);
extern void hrx_semaphore_release(hrx_semaphore_t);
extern hrx_status_t hrx_semaphore_query(hrx_semaphore_t, uint64_t *);

extern hrx_status_t hrx_queue_barrier(hrx_device_t, uint64_t affinity,
                                      const hrx_semaphore_list_t *, const hrx_semaphore_list_t *);
extern hrx_status_t hrx_queue_fill(hrx_device_t, uint64_t, const hrx_semaphore_list_t *,
                                   const hrx_semaphore_list_t *, hrx_buffer_t, size_t, size_t,
                                   const void *, size_t);
extern hrx_status_t hrx_queue_copy(hrx_device_t, uint64_t, const hrx_semaphore_list_t *,
                                   const hrx_semaphore_list_t *, hrx_buffer_t, size_t,
                                   hrx_buffer_t, size_t, size_t);
extern hrx_status_t hrx_queue_host_call(hrx_device_t, uint64_t, const hrx_semaphore_list_t *,
                                        const hrx_semaphore_list_t *, hrx_host_call_fn_t, void *);

static int g_fail = 0;
static void check(const char *n, int pass, const char *d) {
  printf("CHECK %s %s %s\n", n, pass ? "PASS" : "FAIL", d ? d : "");
  if (!pass) g_fail++;
}

static int g_host_called = 0;
static hrx_status_t my_host_call(void *user_data) {
  g_host_called = *(int *)user_data;
  return NULL; // OK
}

int main(void) {
  char d[96];
  hrx_cpu_initialize(0);
  hrx_device_t dev = NULL; hrx_cpu_device_get(0, &dev);

  // --- queue_barrier: signal a semaphore via a barrier, then verify it reached.
  hrx_semaphore_t sem = NULL;
  hrx_semaphore_create(dev, 0, &sem);
  uint64_t sigval = 1;
  hrx_semaphore_list_t sig = { .semaphores = &sem, .values = &sigval, .count = 1 };
  hrx_status_t s = hrx_queue_barrier(dev, 0, NULL, &sig);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(s));
  check("queue_barrier", s == NULL, d);
  if (s == NULL) {
    uint64_t v = 0; hrx_semaphore_query(sem, &v);
    snprintf(d, sizeof d, "sem=%llu", (unsigned long long)v);
    check("queue_barrier_signaled", v >= 1, d);
  } else hrx_status_ignore(s);

  // null device -> INVALID_ARGUMENT (3)
  hrx_status_t e = hrx_queue_barrier(NULL, 0, NULL, NULL);
  check("queue_barrier_null_errors", hrx_status_code(e) == 3, "");
  hrx_status_ignore(e);

  // --- queue_host_call: invoke a host callback, signal a semaphore, verify ---
  hrx_semaphore_t hsem = NULL;
  hrx_semaphore_create(dev, 0, &hsem);
  uint64_t hsv = 1;
  hrx_semaphore_list_t hsig = { .semaphores = &hsem, .values = &hsv, .count = 1 };
  int marker = 0x7E;
  s = hrx_queue_host_call(dev, 0, NULL, &hsig, my_host_call, &marker);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(s));
  check("queue_host_call", s == NULL, d);
  if (s == NULL) {
    // host call runs async on the queue; wait for the signal then check marker.
    uint64_t v = 0;
    for (int i = 0; i < 100000 && v < 1; ++i) hrx_semaphore_query(hsem, &v);
    snprintf(d, sizeof d, "called=0x%x sem=%llu", g_host_called, (unsigned long long)v);
    check("host_call_ran", g_host_called == 0x7E, d);
  } else hrx_status_ignore(s);
  hrx_status_t he = hrx_queue_host_call(dev, 0, NULL, NULL, NULL, NULL);
  check("host_call_null_errors", hrx_status_code(he) == 3, "");
  hrx_status_ignore(he);

  // --- queue_fill / copy: exercise the API; on local-task these may be rejected
  // (QUEUE_TRANSFER usage), but C and Rust must return the SAME status. We print
  // the code (normalized) rather than asserting success.
  hrx_buffer_params_t p = {
      .type = HRX_MEMORY_TYPE_HOST_LOCAL | HRX_MEMORY_TYPE_HOST_VISIBLE,
      .access = HRX_MEMORY_ACCESS_ALL,
      .usage = HRX_BUFFER_USAGE_DEFAULT | HRX_BUFFER_USAGE_MAPPING_SCOPED,
      .queue_affinity = 0};
  hrx_buffer_t b = NULL, b2 = NULL;
  hrx_allocator_allocate_buffer(hrx_device_allocator(dev), p, 256, &b);
  hrx_allocator_allocate_buffer(hrx_device_allocator(dev), p, 256, &b2);
  uint32_t pat = 0xDEADBEEFu;
  hrx_status_t fs = hrx_queue_fill(dev, 0, NULL, NULL, b, 0, 256, &pat, 4);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(fs));
  check("queue_fill_status", 1, d);
  if (fs) hrx_status_ignore(fs);
  hrx_status_t cs = hrx_queue_copy(dev, 0, NULL, NULL, b, 0, b2, 0, 256);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(cs));
  check("queue_copy_status", 1, d);
  if (cs) hrx_status_ignore(cs);

  hrx_buffer_release(b); hrx_buffer_release(b2);
  hrx_semaphore_release(sem); hrx_semaphore_release(hsem);
  s = hrx_cpu_shutdown();
  check("cpu_shutdown", s == NULL, "");

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
