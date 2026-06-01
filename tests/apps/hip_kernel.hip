// A HIP app with a real kernel launch, to exercise the buffer tracer's
// kernel-name tracking and buffer-dump (hex/hash) paths. Built with hipcc.
#include <hip/hip_runtime.h>
#include <cstdio>

__global__ void fill_iota(int *out, int n) {
  int i = blockIdx.x * blockDim.x + threadIdx.x;
  if (i < n)
    out[i] = i * 2;
}

int main() {
  int count = 0;
  hipGetDeviceCount(&count);
  if (count <= 0) {
    printf("no device\n");
    return 0;
  }
  hipSetDevice(0);

  const int n = 64;
  int *d = nullptr;
  hipMalloc(&d, n * sizeof(int));
  hipMemset(d, 0, n * sizeof(int));

  hipLaunchKernelGGL(fill_iota, dim3(1), dim3(n), 0, 0, d, n);
  hipDeviceSynchronize();

  int host[n];
  hipMemcpy(host, d, n * sizeof(int), hipMemcpyDeviceToHost);
  printf("result[10]=%d result[63]=%d\n", host[10], host[63]);

  hipFree(d);
  return 0;
}
