// Differential test for the init-free libhrx ABI (status, host_allocator).
// Links against EITHER the C libhrx.so OR the Rust libhrx_rs.so; the same
// binary must produce identical output for both. No GPU and no HRX init needed.
//
// NOTE: value_list is intentionally NOT exercised here — hrx_value_list_*
// requires hrx_cpu_initialize() first (it uses the VM instance set up by init),
// so it is not init-free. It is covered separately once the init/device modules
// are ported (see scripts/libhrx_diff_test.sh).
//
// Emits machine-checkable lines: `CHECK <name> <PASS|FAIL> <detail>`.
#include <stdint.h>
#include <stdio.h>
#include <string.h>

// --- hrx public ABI (subset under test); self-declared so we can link either
//     backend .so without its headers. ---
typedef struct hrx_status_s *hrx_status_t;
typedef struct hrx_value_list_s *hrx_value_list_t;
typedef struct hrx_host_allocator_t {
  void *self;
  void *ctl;
} hrx_host_allocator_t;

enum {
  HRX_STATUS_OK = 0,
  HRX_STATUS_INVALID_ARGUMENT = 3,
  HRX_STATUS_NOT_FOUND = 5,
  HRX_STATUS_OUT_OF_RANGE = 11,
};

extern hrx_host_allocator_t hrx_host_allocator_system_value;
static inline hrx_host_allocator_t hrx_host_allocator_system(void) {
  return hrx_host_allocator_system_value;
}

extern hrx_status_t hrx_make_status(int code, const char *message);
extern int hrx_status_code(hrx_status_t status);
extern hrx_status_t hrx_status_to_string(hrx_status_t status, char **out_message,
                                         size_t *out_length);
extern void hrx_status_free_message(char *message);
extern void hrx_status_ignore(hrx_status_t status);

extern hrx_status_t hrx_host_allocator_malloc(hrx_host_allocator_t a, size_t n,
                                              void **out);
extern hrx_status_t hrx_host_allocator_malloc_uninitialized(
    hrx_host_allocator_t a, size_t n, void **out);
extern hrx_status_t hrx_host_allocator_realloc(hrx_host_allocator_t a, size_t n,
                                               void **inout);
extern hrx_status_t hrx_host_allocator_clone(hrx_host_allocator_t a,
                                             const void *src, size_t n,
                                             void **out);
extern void hrx_host_allocator_free(hrx_host_allocator_t a, void *p);
extern hrx_status_t hrx_host_allocator_malloc_aligned(hrx_host_allocator_t a,
                                                      size_t n, size_t align,
                                                      size_t off, void **out);
extern void hrx_host_allocator_free_aligned(hrx_host_allocator_t a, void *p);

static int g_fail = 0;
static void check(const char *name, int pass, const char *detail) {
  printf("CHECK %s %s %s\n", name, pass ? "PASS" : "FAIL", detail ? detail : "");
  if (!pass) g_fail++;
}

int main(void) {
  // --- status ---
  {
    hrx_status_t ok = hrx_make_status(HRX_STATUS_OK, "ignored");
    check("make_status_ok_is_null", ok == NULL, "");
    check("status_code_ok", hrx_status_code(ok) == HRX_STATUS_OK, "");

    hrx_status_t err = hrx_make_status(HRX_STATUS_NOT_FOUND, "thing missing");
    char d[64];
    snprintf(d, sizeof d, "code=%d", hrx_status_code(err));
    check("status_code_err", hrx_status_code(err) == HRX_STATUS_NOT_FOUND, d);

    char *msg = NULL;
    size_t len = 0;
    hrx_status_t s = hrx_status_to_string(err, &msg, &len);
    snprintf(d, sizeof d, "msg='%s' len=%zu", msg ? msg : "(null)", len);
    check("status_to_string_err",
          s == NULL && msg && strcmp(msg, "thing missing") == 0 &&
              len == strlen("thing missing"),
          d);
    hrx_status_free_message(msg);

    msg = NULL; len = 0;
    s = hrx_status_to_string(ok, &msg, &len);
    snprintf(d, sizeof d, "msg='%s' len=%zu", msg ? msg : "(null)", len);
    check("status_to_string_ok",
          s == NULL && msg && strcmp(msg, "OK") == 0 && len == 2, d);
    hrx_status_free_message(msg);

    hrx_status_t bad = hrx_status_to_string(err, NULL, NULL);
    check("status_to_string_null_out",
          bad != NULL && hrx_status_code(bad) == HRX_STATUS_INVALID_ARGUMENT, "");
    hrx_status_ignore(bad);

    hrx_status_ignore(err);  // frees payload; must not crash
    check("status_ignore_ok_noop", (hrx_status_ignore(ok), 1), "");
  }

  // --- host allocator ---
  {
    hrx_host_allocator_t a = hrx_host_allocator_system();
    check("host_alloc_system_nonnull_ctl", a.ctl != NULL, "");

    void *p = NULL;
    hrx_status_t s = hrx_host_allocator_malloc(a, 1024, &p);
    check("host_malloc", s == NULL && p != NULL, "");
    if (p) {
      memset(p, 0xAB, 1024);
      s = hrx_host_allocator_realloc(a, 4096, &p);
      check("host_realloc", s == NULL && p != NULL, "");
      hrx_host_allocator_free(a, p);
    }

    void *q = NULL;
    s = hrx_host_allocator_malloc_uninitialized(a, 256, &q);
    check("host_malloc_uninit", s == NULL && q != NULL, "");
    hrx_host_allocator_free(a, q);

    const char src[16] = "hello-clone-123";
    void *c = NULL;
    s = hrx_host_allocator_clone(a, src, sizeof src, &c);
    check("host_clone", s == NULL && c != NULL && memcmp(c, src, sizeof src) == 0, "");
    hrx_host_allocator_free(a, c);

    void *al = NULL;
    s = hrx_host_allocator_malloc_aligned(a, 1000, 256, 0, &al);
    char d[64];
    snprintf(d, sizeof d, "ptr%%256=%zu", al ? ((size_t)al) % 256 : 999);
    check("host_malloc_aligned",
          s == NULL && al != NULL && ((size_t)al % 256) == 0, d);
    hrx_host_allocator_free_aligned(a, al);
  }

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
