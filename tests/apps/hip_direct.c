// Exercises direct-passthrough HIP functions (the ones that bypass the
// interceptor table and forward straight to the backend via dlsym). Used to
// confirm the Rust passthrough forwards them identically to the C passthrough.
//
// Prints results to stdout in a stable, address-free format so the C and Rust
// passthrough outputs can be compared byte-for-byte.
#include <stdio.h>
#include <stddef.h>

typedef int hipError_t;
extern hipError_t hipGetDeviceCount(int *count);
extern hipError_t hipSetDevice(int id);
extern hipError_t hipDeviceTotalMem(size_t *bytes, int id);          // direct
extern hipError_t hipDeviceGetPCIBusId(char *s, int len, int id);    // direct
extern hipError_t hipDeviceComputeCapability(int *major, int *minor, int id); // direct
extern const char *hipGetErrorName(hipError_t e);                    // table-routed

int main(void) {
  int count = 0;
  hipGetDeviceCount(&count);
  printf("count=%d\n", count);
  if (count <= 0)
    return 0;
  hipSetDevice(0);

  size_t total = 0;
  hipError_t e1 = hipDeviceTotalMem(&total, 0);
  // Print in GiB rounded to avoid tiny variation; total is deterministic anyway.
  printf("totalMem_ret=%d totalMem_GiB=%zu\n", e1, total >> 30);

  char busid[64] = {0};
  hipError_t e2 = hipDeviceGetPCIBusId(busid, sizeof(busid), 0);
  printf("pciBusId_ret=%d pciBusId=%s\n", e2, busid);

  int major = 0, minor = 0;
  hipError_t e3 = hipDeviceComputeCapability(&major, &minor, 0);
  printf("computeCap_ret=%d cap=%d.%d\n", e3, major, minor);

  printf("errName0=%s\n", hipGetErrorName(0));
  return 0;
}
