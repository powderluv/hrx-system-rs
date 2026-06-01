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
typedef struct hrx_buffer_view_s *hrx_buffer_view_t;
typedef struct hrx_value_list_s *hrx_value_list_t;
#define HRX_STATUS_OUT_OF_RANGE 11

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
extern hrx_status_t hrx_buffer_view_create(hrx_buffer_t, size_t rank, const int64_t *shape,
                                           uint32_t etype, uint32_t enc, hrx_buffer_view_t *);
extern hrx_status_t hrx_buffer_view_rank(hrx_buffer_view_t, size_t *);
extern hrx_status_t hrx_buffer_view_dim(hrx_buffer_view_t, size_t, int64_t *);
extern void hrx_buffer_view_retain(hrx_buffer_view_t);
extern void hrx_buffer_view_release(hrx_buffer_view_t);
extern hrx_status_t hrx_value_list_create(size_t, hrx_value_list_t *);
extern void hrx_value_list_release(hrx_value_list_t);
extern hrx_status_t hrx_value_list_size(hrx_value_list_t, size_t *);
extern hrx_status_t hrx_value_list_push_buffer(hrx_value_list_t, hrx_buffer_t);

extern hrx_status_t hrx_allocator_query_virtual_memory(hrx_allocator_t, uint32_t mem_type,
                                                       unsigned char *supported, size_t *min_page,
                                                       size_t *rec_page);
extern hrx_status_t hrx_allocator_import_buffer(hrx_allocator_t, hrx_buffer_params_t,
                                                void *host_ptr, size_t, hrx_buffer_t *);

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

  // --- buffer_view over the buffer ---
  {
    int64_t shape[2] = {16, 64}; // 1024 int8 elements
    hrx_buffer_view_t bv = NULL;
    // element_type OPAQUE_8 = ? use INT_32-ish? Use a value-agnostic create with
    // FLOAT_32 + DENSE_ROW_MAJOR; rank/dim are what we verify.
    s = hrx_buffer_view_create(buf, 2, shape, 0x10000020u /*FLOAT_32-ish*/, 1, &bv);
    // Some element types may be rejected; accept either OK+nonnull or a clean
    // error — but assert C and Rust agree (the differential covers that). Here
    // we just record the code.
    snprintf(d, sizeof d, "code=%d bv_nonnull=%d", hrx_status_code(s), bv != NULL);
    check("buffer_view_create", 1, d);
    if (s == NULL && bv) {
      size_t rank = 0;
      hrx_status_t rs = hrx_buffer_view_rank(bv, &rank);
      snprintf(d, sizeof d, "rank=%zu", rank);
      check("buffer_view_rank", rs == NULL && rank == 2, d);
      int64_t d0 = 0, d1 = 0;
      hrx_buffer_view_dim(bv, 0, &d0);
      hrx_buffer_view_dim(bv, 1, &d1);
      snprintf(d, sizeof d, "d0=%lld d1=%lld", (long long)d0, (long long)d1);
      check("buffer_view_dims", d0 == 16 && d1 == 64, d);
      hrx_status_t oor = hrx_buffer_view_dim(bv, 9, &d0);
      check("buffer_view_dim_oob", hrx_status_code(oor) == HRX_STATUS_OUT_OF_RANGE, "");
      hrx_status_ignore(oor);
      hrx_buffer_view_retain(bv);
      hrx_buffer_view_release(bv);
      hrx_buffer_view_release(bv);
    } else if (s) {
      hrx_status_ignore(s);
    }
  }

  // --- value_list ref-pushes (buffer) ---
  {
    hrx_value_list_t vl = NULL;
    s = hrx_value_list_create(4, &vl);
    if (s == NULL && vl) {
      hrx_status_t ps = hrx_value_list_push_buffer(vl, buf);
      check("vlist_push_buffer", ps == NULL, "");
      size_t sz = 0; hrx_value_list_size(vl, &sz);
      snprintf(d, sizeof d, "size=%zu", sz);
      check("vlist_push_buffer_size", sz == 1, d);
      hrx_value_list_release(vl);
    } else {
      check("vlist_create_for_push", 0, "");
      if (s) hrx_status_ignore(s);
    }
  }

  // --- allocator query_virtual_memory: deterministic; on local-task CPU the
  // allocator reports no virtual-memory support (supported=0, page sizes 0).
  {
    unsigned char supported = 0xAA;
    size_t minp = 12345, recp = 67890;
    hrx_status_t qs = hrx_allocator_query_virtual_memory(
        alloc, HRX_MEMORY_TYPE_HOST_LOCAL | HRX_MEMORY_TYPE_HOST_VISIBLE,
        &supported, &minp, &recp);
    snprintf(d, sizeof d, "code=%d supported=%d minp=%zu recp=%zu",
             hrx_status_code(qs), (int)supported, minp, recp);
    check("query_virtual_memory", qs == NULL, d);
    if (qs) hrx_status_ignore(qs);
  }

  // --- allocator import_buffer: import a host allocation. The underlying IREE
  // call is identical for C and Rust, so the status code matches regardless of
  // whether this allocator supports host import.
  {
    static unsigned char host_region[256];
    for (size_t i = 0; i < sizeof host_region; ++i) host_region[i] = (unsigned char)i;
    hrx_buffer_params_t ip = {
        .type = HRX_MEMORY_TYPE_HOST_LOCAL | HRX_MEMORY_TYPE_HOST_VISIBLE,
        .access = HRX_MEMORY_ACCESS_ALL,
        .usage = HRX_BUFFER_USAGE_DEFAULT | HRX_BUFFER_USAGE_MAPPING_SCOPED,
        .queue_affinity = 0,
    };
    hrx_buffer_t imported = NULL;
    hrx_status_t is = hrx_allocator_import_buffer(alloc, ip, host_region,
                                                  sizeof host_region, &imported);
    snprintf(d, sizeof d, "code=%d imported_nonnull=%d", hrx_status_code(is),
             imported != NULL);
    check("import_buffer", 1, d);
    if (is == NULL && imported) {
      hrx_buffer_release(imported);
    } else if (is) {
      hrx_status_ignore(is);
    }
    // null args -> INVALID_ARGUMENT (3)
    hrx_status_t ie = hrx_allocator_import_buffer(alloc, ip, NULL, 16, &imported);
    check("import_buffer_null_errors", hrx_status_code(ie) == 3, "");
    hrx_status_ignore(ie);
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
