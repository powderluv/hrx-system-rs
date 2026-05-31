// Minimal HIP smoke application for exercising the LD_PRELOAD passthrough path.
//
// Self-declares the handful of HIP entry points it uses so it builds without
// ROCm headers. At runtime the symbols resolve to whatever provides them: the
// LD_PRELOAD'd passthrough (libhip_intercept.so / Rust libamdhip64.so), which
// forwards to the real backend libamdhip64.so given by HIP_PASSTHROUGH_BACKEND_LIB.
//
// The call sequence is deterministic so the interceptor trace can be diffed
// between the C reference build and the Rust port.
#include <stddef.h>
#include <stdio.h>

typedef int hipError_t;
extern hipError_t hipInit(unsigned int flags);
extern hipError_t hipDriverGetVersion(int *v);
extern hipError_t hipRuntimeGetVersion(int *v);
extern hipError_t hipGetDeviceCount(int *count);
extern hipError_t hipSetDevice(int id);
extern hipError_t hipDeviceGetName(char *name, int len, int id);
extern hipError_t hipMalloc(void **ptr, size_t size);
extern hipError_t hipMemcpy(void *dst, const void *src, size_t n, int kind);
extern hipError_t hipMemset(void *dst, int value, size_t n);
extern hipError_t hipFree(void *ptr);
extern hipError_t hipDeviceSynchronize(void);

#define H2D 1
#define D2H 2

int main(void) {
  hipInit(0);

  int driver = 0, runtime = 0;
  hipDriverGetVersion(&driver);
  hipRuntimeGetVersion(&runtime);

  int count = 0;
  hipGetDeviceCount(&count);
  fprintf(stderr, "[app] device count = %d\n", count);

  if (count > 0) {
    hipSetDevice(0);
    char name[256] = {0};
    hipDeviceGetName(name, sizeof(name), 0);
    fprintf(stderr, "[app] device 0 = %s\n", name);

    const size_t n = 1024;
    void *d = NULL;
    if (hipMalloc(&d, n) == 0 && d) {
      char host[1024];
      for (size_t i = 0; i < n; ++i) host[i] = (char)i;
      hipMemcpy(d, host, n, H2D);
      hipMemset(d, 0, n);
      hipMemcpy(host, d, n, D2H);
      hipFree(d);
    }
    hipDeviceSynchronize();
  }

  fprintf(stderr, "[app] done\n");
  return 0;
}
