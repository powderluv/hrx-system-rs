// Differential test for the libhrx GPU accelerator path (hrx_gpu_*). Links the
// C libhrx.so or Rust libhrx_rs.so; identical output. REQUIRES a GPU with
// fine-grained device-local memory (e.g. MI300X/gfx942) — run on the MI300 with
// HRX_GPU_DRIVER=amdgpu. The device name contains a serial/node suffix that
// varies, so we normalize: print only whether the name is non-empty + the arch.
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct hrx_status_s *hrx_status_t;
typedef struct hrx_device_s *hrx_device_t;

enum { HRX_DEVICE_PROPERTY_NAME = 0, HRX_DEVICE_PROPERTY_ARCHITECTURE = 1 };
enum { HRX_ACCELERATOR_GPU = 0 };

extern hrx_status_t hrx_gpu_initialize(uint32_t);
extern hrx_status_t hrx_gpu_shutdown(void);
extern hrx_status_t hrx_gpu_device_count(int *);
extern hrx_status_t hrx_gpu_device_get(int, hrx_device_t *);
extern hrx_status_t hrx_device_get_property(hrx_device_t, int, void *, size_t);
extern hrx_status_t hrx_device_get_type(hrx_device_t, int *);
extern hrx_status_t hrx_device_synchronize(hrx_device_t);
extern int hrx_status_code(hrx_status_t);
extern void hrx_status_ignore(hrx_status_t);

static int g_fail = 0;
static void check(const char *n, int pass, const char *d) {
  printf("CHECK %s %s %s\n", n, pass ? "PASS" : "FAIL", d ? d : "");
  if (!pass) g_fail++;
}

int main(void) {
  char d[160];
  // device count before init -> UNAVAILABLE (14)
  int c = -1;
  hrx_status_t pre = hrx_gpu_device_count(&c);
  check("count_before_init_errors", hrx_status_code(pre) == 14, "");
  hrx_status_ignore(pre);

  hrx_status_t s = hrx_gpu_initialize(0);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(s));
  check("gpu_initialize", s == NULL, d);
  if (s) { hrx_status_ignore(s); printf("SUMMARY %s failures=%d\n", g_fail?"FAIL":"PASS", g_fail); return g_fail?1:0; }

  // double init -> ALREADY_EXISTS (6)
  hrx_status_t s2 = hrx_gpu_initialize(0);
  check("gpu_double_init_errors", hrx_status_code(s2) == 6, "");
  hrx_status_ignore(s2);

  c = 0;
  s = hrx_gpu_device_count(&c);
  snprintf(d, sizeof d, "count_positive=%d", c > 0);
  check("gpu_device_count", s == NULL && c > 0, d);

  hrx_device_t dev = NULL;
  s = hrx_gpu_device_get(0, &dev);
  check("gpu_device_get", s == NULL && dev != NULL, "");

  // out-of-range -> OUT_OF_RANGE (11)
  hrx_device_t bad = NULL;
  hrx_status_t oor = hrx_gpu_device_get(9999, &bad);
  check("gpu_device_get_oob", hrx_status_code(oor) == 11, "");
  hrx_status_ignore(oor);

  int type = -1;
  s = hrx_device_get_type(dev, &type);
  check("gpu_device_type", s == NULL && type == HRX_ACCELERATOR_GPU, "");

  char name[256] = {0};
  s = hrx_device_get_property(dev, HRX_DEVICE_PROPERTY_NAME, name, sizeof name);
  snprintf(d, sizeof d, "name_nonempty=%d starts_AMD=%d", (int)(strlen(name) > 0),
           strncmp(name, "AMD", 3) == 0);
  check("gpu_device_name", s == NULL && strlen(name) > 0, d);

  char arch[64] = {0};
  s = hrx_device_get_property(dev, HRX_DEVICE_PROPERTY_ARCHITECTURE, arch, sizeof arch);
  snprintf(d, sizeof d, "arch=%s", arch);
  // arch is gfxNNN (deterministic for the device); print it (same on both backends)
  check("gpu_device_arch", s == NULL && strncmp(arch, "gfx", 3) == 0, d);

  s = hrx_device_synchronize(dev);
  check("gpu_device_synchronize", s == NULL, "");

  s = hrx_gpu_shutdown();
  check("gpu_shutdown", s == NULL, "");

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
