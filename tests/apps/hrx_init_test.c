// Differential test for the libhrx init/CPU/device/value_list path. Links
// against EITHER the C libhrx.so OR the Rust libhrx_rs.so; identical output.
// Uses the CPU (local-task) accelerator — no GPU required.
//
// Emits `CHECK <name> <PASS|FAIL> <detail>`.
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct hrx_status_s *hrx_status_t;
typedef struct hrx_device_s *hrx_device_t;
typedef struct hrx_value_list_s *hrx_value_list_t;

enum { HRX_STATUS_OK = 0, HRX_STATUS_OUT_OF_RANGE = 11 };
enum {
  HRX_DEVICE_PROPERTY_NAME = 0,
  HRX_DEVICE_PROPERTY_ARCHITECTURE = 1,
  HRX_DEVICE_PROPERTY_TOTAL_MEMORY = 2,
  HRX_DEVICE_PROPERTY_COMPUTE_UNITS = 3,
};
enum { HRX_ACCELERATOR_CPU = 1 };

extern hrx_status_t hrx_cpu_initialize(uint32_t flags);
extern hrx_status_t hrx_cpu_shutdown(void);
extern hrx_status_t hrx_cpu_device_count(int *count);
extern hrx_status_t hrx_cpu_device_get(int index, hrx_device_t *device);
extern hrx_status_t hrx_device_get_property(hrx_device_t d, int prop, void *v, size_t n);
extern hrx_status_t hrx_device_get_type(hrx_device_t d, int *type);
extern hrx_status_t hrx_device_synchronize(hrx_device_t d);
extern int hrx_status_code(hrx_status_t s);
extern void hrx_status_ignore(hrx_status_t s);
extern void hrx_runtime_version(int *a, int *b, int *c);

extern hrx_status_t hrx_value_list_create(size_t capacity, hrx_value_list_t *list);
extern void hrx_value_list_release(hrx_value_list_t list);
extern hrx_status_t hrx_value_list_size(hrx_value_list_t list, size_t *size);
extern hrx_status_t hrx_value_list_push_i64(hrx_value_list_t list, int64_t v);
extern hrx_status_t hrx_value_list_get_i64(hrx_value_list_t list, size_t i, int64_t *v);
extern hrx_status_t hrx_value_list_push_null_ref(hrx_value_list_t list);

static int g_fail = 0;
static void check(const char *name, int pass, const char *detail) {
  printf("CHECK %s %s %s\n", name, pass ? "PASS" : "FAIL", detail ? detail : "");
  if (!pass) g_fail++;
}

int main(void) {
  int vmaj = -1, vmin = -1, vpat = -1;
  hrx_runtime_version(&vmaj, &vmin, &vpat);
  char d[256];
  snprintf(d, sizeof d, "%d.%d.%d", vmaj, vmin, vpat);
  check("runtime_version", vmaj == 0 && vmin == 1 && vpat == 0, d);

  // device count before init must be UNAVAILABLE (14)
  int c = -123;
  hrx_status_t s = hrx_cpu_device_count(&c);
  check("count_before_init_errors", hrx_status_code(s) == 14, "");
  hrx_status_ignore(s);

  s = hrx_cpu_initialize(0);
  snprintf(d, sizeof d, "code=%d", hrx_status_code(s));
  check("cpu_initialize", s == NULL, d);

  // double init -> ALREADY_EXISTS (6)
  hrx_status_t s2 = hrx_cpu_initialize(0);
  check("cpu_double_init_errors", hrx_status_code(s2) == 6, "");
  hrx_status_ignore(s2);

  c = -1;
  s = hrx_cpu_device_count(&c);
  snprintf(d, sizeof d, "count=%d", c);
  check("cpu_device_count", s == NULL && c == 1, d);

  hrx_device_t dev = NULL;
  s = hrx_cpu_device_get(0, &dev);
  check("cpu_device_get", s == NULL && dev != NULL, "");

  // out-of-range index -> OUT_OF_RANGE (11)
  hrx_device_t bad = NULL;
  hrx_status_t oor = hrx_cpu_device_get(5, &bad);
  check("cpu_device_get_oob", hrx_status_code(oor) == HRX_STATUS_OUT_OF_RANGE, "");
  hrx_status_ignore(oor);

  int type = -1;
  s = hrx_device_get_type(dev, &type);
  check("device_type_cpu", s == NULL && type == HRX_ACCELERATOR_CPU, "");

  char name[128] = {0};
  s = hrx_device_get_property(dev, HRX_DEVICE_PROPERTY_NAME, name, sizeof name);
  snprintf(d, sizeof d, "name='%s'", name);
  check("device_name", s == NULL && strcmp(name, "CPU 0 (local-task)") == 0, d);

  char arch[64] = {0};
  s = hrx_device_get_property(dev, HRX_DEVICE_PROPERTY_ARCHITECTURE, arch, sizeof arch);
  snprintf(d, sizeof d, "arch='%s'", arch);
  check("device_arch", s == NULL && strcmp(arch, "host") == 0, d);

  // name into too-small buffer -> OUT_OF_RANGE
  char tiny[4];
  hrx_status_t tn = hrx_device_get_property(dev, HRX_DEVICE_PROPERTY_NAME, tiny, sizeof tiny);
  check("device_name_too_small", hrx_status_code(tn) == HRX_STATUS_OUT_OF_RANGE, "");
  hrx_status_ignore(tn);

  uint32_t cu = 12345;
  s = hrx_device_get_property(dev, HRX_DEVICE_PROPERTY_COMPUTE_UNITS, &cu, sizeof cu);
  snprintf(d, sizeof d, "cu=%u", cu);
  check("device_compute_units", s == NULL && cu == 0, d);

  s = hrx_device_synchronize(dev);
  check("device_synchronize", s == NULL, "");

  // value_list now works (VM instance is up)
  {
    hrx_value_list_t list = NULL;
    s = hrx_value_list_create(8, &list);
    check("vlist_create", s == NULL && list != NULL, "");
    int64_t vals[] = {0, 1, -1, 42, 9223372036854775807LL};
    int ok = 1;
    for (size_t i = 0; i < sizeof vals / sizeof *vals; ++i)
      if (hrx_value_list_push_i64(list, vals[i]) != NULL) ok = 0;
    check("vlist_push_i64", ok, "");
    hrx_value_list_push_null_ref(list);
    size_t sz = 0; hrx_value_list_size(list, &sz);
    snprintf(d, sizeof d, "size=%zu", sz);
    check("vlist_size", sz == 6, d);
    int get_ok = 1;
    for (size_t i = 0; i < sizeof vals / sizeof *vals; ++i) {
      int64_t got = 0;
      if (hrx_value_list_get_i64(list, i, &got) != NULL || got != vals[i]) get_ok = 0;
    }
    check("vlist_get_roundtrip", get_ok, "");
    hrx_value_list_release(list);
  }

  s = hrx_cpu_shutdown();
  check("cpu_shutdown", s == NULL, "");

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
