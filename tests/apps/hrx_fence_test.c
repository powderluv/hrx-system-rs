// Differential test for fence (full) + module/executable error+lifecycle paths.
// Links against C libhrx.so or Rust libhrx_rs.so; identical output. CPU accel.
// Module/executable successful-load isn't tested (needs a compiled VMFB /
// executable we can't generate without the IREE compiler) — we cover the
// null-arg + invalid-data error paths, which must match between backends.
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct hrx_status_s *hrx_status_t;
typedef struct hrx_device_s *hrx_device_t;
typedef struct hrx_semaphore_s *hrx_semaphore_t;
typedef struct hrx_fence_s *hrx_fence_t;
typedef struct hrx_module_s *hrx_module_t;
typedef struct hrx_function_s *hrx_function_t;
typedef struct hrx_executable_s *hrx_executable_t;

extern hrx_status_t hrx_cpu_initialize(uint32_t);
extern hrx_status_t hrx_cpu_shutdown(void);
extern hrx_status_t hrx_cpu_device_get(int, hrx_device_t *);
extern int hrx_status_code(hrx_status_t);
extern void hrx_status_ignore(hrx_status_t);
extern hrx_status_t hrx_semaphore_create(hrx_device_t, uint64_t, hrx_semaphore_t *);
extern void hrx_semaphore_release(hrx_semaphore_t);
extern hrx_status_t hrx_semaphore_signal(hrx_semaphore_t, uint64_t);

extern hrx_status_t hrx_fence_create(size_t, hrx_fence_t *);
extern hrx_status_t hrx_fence_create_at(hrx_semaphore_t, uint64_t, hrx_fence_t *);
extern void hrx_fence_retain(hrx_fence_t);
extern void hrx_fence_release(hrx_fence_t);
extern hrx_status_t hrx_fence_insert(hrx_fence_t, hrx_semaphore_t, uint64_t);
extern hrx_status_t hrx_fence_extend(hrx_fence_t, hrx_fence_t);
extern hrx_status_t hrx_fence_signal(hrx_fence_t);
extern hrx_status_t hrx_fence_wait(hrx_fence_t, uint64_t);

extern hrx_status_t hrx_module_load_vmfb(hrx_device_t, const void *, size_t, hrx_module_t *);
extern hrx_status_t hrx_module_lookup_function(hrx_module_t, const char *, hrx_function_t *);
extern hrx_status_t hrx_executable_load_data(hrx_device_t, const void *, size_t, const char *, hrx_executable_t *);
extern hrx_status_t hrx_executable_load_file(hrx_device_t, const char *path, const char *fmt, hrx_executable_t *);

static int g_fail = 0;
static void check(const char *n, int pass, const char *d) {
  printf("CHECK %s %s %s\n", n, pass ? "PASS" : "FAIL", d ? d : "");
  if (!pass) g_fail++;
}

int main(void) {
  char d[64];
  hrx_cpu_initialize(0);
  hrx_device_t dev = NULL; hrx_cpu_device_get(0, &dev);

  // --- fence: create, signal an empty fence, wait (immediate), lifecycle ---
  hrx_fence_t f = NULL;
  hrx_status_t s = hrx_fence_create(4, &f);
  check("fence_create", s == NULL && f != NULL, "");
  // an empty fence (no semaphores inserted) is trivially reached → wait OK
  s = hrx_fence_wait(f, 0);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(s));
  check("fence_wait_empty", s == NULL, d);
  if (s) hrx_status_ignore(s);
  s = hrx_fence_signal(f);
  check("fence_signal", s == NULL, "");

  // fence_create_at a semaphore value, then insert into another fence + extend
  hrx_semaphore_t sem = NULL;
  hrx_semaphore_create(dev, 0, &sem);
  hrx_semaphore_signal(sem, 7);
  hrx_fence_t fat = NULL;
  s = hrx_fence_create_at(sem, 7, &fat);
  check("fence_create_at", s == NULL && fat != NULL, "");
  s = hrx_fence_wait(fat, 0); // value 7 already signaled → reached
  snprintf(d, sizeof d, "code=%d", hrx_status_code(s));
  check("fence_at_wait_reached", s == NULL, d);
  if (s) hrx_status_ignore(s);

  hrx_fence_t f2 = NULL; hrx_fence_create(4, &f2);
  s = hrx_fence_insert(f2, sem, 7);
  check("fence_insert", s == NULL, "");
  s = hrx_fence_extend(f, f2);
  check("fence_extend", s == NULL, "");

  // null-arg error paths (INVALID_ARGUMENT=3)
  hrx_status_t e = hrx_fence_create(4, NULL);
  check("fence_create_null_errors", hrx_status_code(e) == 3, "");
  hrx_status_ignore(e);

  hrx_fence_retain(f);
  hrx_fence_release(f);
  hrx_fence_release(f);
  hrx_fence_release(f2);
  hrx_fence_release(fat);
  hrx_semaphore_release(sem);
  check("fence_lifecycle", 1, "");

  // --- module: invalid-data + null-arg error paths ---
  hrx_module_t mod = NULL;
  unsigned char garbage[64];
  memset(garbage, 0xCC, sizeof garbage);
  hrx_status_t me = hrx_module_load_vmfb(dev, garbage, sizeof garbage, &mod);
  check("module_load_garbage_errors", me != NULL && mod == NULL, "");
  if (me) hrx_status_ignore(me);
  hrx_status_t mn = hrx_module_load_vmfb(dev, NULL, 0, &mod);
  check("module_load_null_errors", hrx_status_code(mn) == 3, "");
  hrx_status_ignore(mn);

  // --- executable: invalid-data + null-arg ---
  hrx_executable_t exe = NULL;
  hrx_status_t xe = hrx_executable_load_data(dev, garbage, sizeof garbage, "invalid-fmt", &exe);
  check("executable_load_garbage_errors", xe != NULL && exe == NULL, "");
  if (xe) hrx_status_ignore(xe);
  hrx_status_t xn = hrx_executable_load_data(dev, NULL, 0, "x", &exe);
  check("executable_load_null_errors", hrx_status_code(xn) == 3, "");
  hrx_status_ignore(xn);

  // --- executable_load_file: error ladder (NULL args=3, missing file=NOT_FOUND=5)
  hrx_status_t fn = hrx_executable_load_file(dev, NULL, "x", &exe);
  check("executable_load_file_null_errors", hrx_status_code(fn) == 3, "");
  hrx_status_ignore(fn);
  hrx_status_t fnf = hrx_executable_load_file(dev, "/nonexistent/hrx/path.bin", "x", &exe);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(fnf));
  check("executable_load_file_missing", hrx_status_code(fnf) == 5, d);
  hrx_status_ignore(fnf);

  s = hrx_cpu_shutdown();
  check("cpu_shutdown", s == NULL, "");

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
