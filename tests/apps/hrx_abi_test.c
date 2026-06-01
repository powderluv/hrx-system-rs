// Differential test for the GPU-independent libhrx ABI (status, host_allocator,
// value_list). Links against EITHER the C libhrx.so OR the Rust libhrx_rs.so;
// the same binary must produce identical output for both. No GPU required.
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

extern hrx_status_t hrx_value_list_create(size_t capacity,
                                          hrx_value_list_t *list);
extern void hrx_value_list_retain(hrx_value_list_t list);
extern void hrx_value_list_release(hrx_value_list_t list);
extern hrx_status_t hrx_value_list_size(hrx_value_list_t list, size_t *size);
extern hrx_status_t hrx_value_list_push_i64(hrx_value_list_t list, int64_t v);
extern hrx_status_t hrx_value_list_get_i64(hrx_value_list_t list, size_t index,
                                           int64_t *v);
extern hrx_status_t hrx_value_list_push_null_ref(hrx_value_list_t list);

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
          s == NULL && msg && strcmp(msg, "thing missing") == 0 && len == 12, d);
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

  // --- value list ---
  {
    hrx_value_list_t list = NULL;
    hrx_status_t s = hrx_value_list_create(8, &list);
    check("vlist_create", s == NULL && list != NULL, "");

    int64_t vals[] = {0, 1, -1, 42, 9223372036854775807LL, -9223372036854775807LL};
    int push_ok = 1;
    for (size_t i = 0; i < sizeof vals / sizeof *vals; ++i)
      if (hrx_value_list_push_i64(list, vals[i]) != NULL) push_ok = 0;
    check("vlist_push_i64", push_ok, "");

    s = hrx_value_list_push_null_ref(list);
    check("vlist_push_null_ref", s == NULL, "");

    size_t sz = 0;
    s = hrx_value_list_size(list, &sz);
    char d[64];
    snprintf(d, sizeof d, "size=%zu", sz);
    check("vlist_size", s == NULL && sz == 7, d);  // 6 i64 + 1 null ref

    int get_ok = 1;
    for (size_t i = 0; i < sizeof vals / sizeof *vals; ++i) {
      int64_t got = 0;
      if (hrx_value_list_get_i64(list, i, &got) != NULL || got != vals[i])
        get_ok = 0;
    }
    check("vlist_get_i64_roundtrip", get_ok, "");

    // out-of-range get should error
    int64_t dummy = 0;
    hrx_status_t oor = hrx_value_list_get_i64(list, 999, &dummy);
    check("vlist_get_oob_errors", oor != NULL, "");
    hrx_status_ignore(oor);

    hrx_value_list_retain(list);
    hrx_value_list_release(list);  // back to 1
    hrx_value_list_release(list);  // frees
    check("vlist_retain_release", 1, "");
  }

  printf("SUMMARY %s failures=%d\n", g_fail ? "FAIL" : "PASS", g_fail);
  return g_fail ? 1 : 0;
}
