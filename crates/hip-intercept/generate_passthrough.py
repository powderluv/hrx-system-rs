#!/usr/bin/env python3
"""Generate Rust trampolines for the direct-passthrough HIP functions.

Parses libhrx/src/passthrough/hip_intercept.c and emits, for every
direct-passthrough function (the ones that resolve a backend symbol via
`dlsym(g_backend_lib, "NAME")` and forward to it, bypassing the interceptor
table), a faithful `#[no_mangle] extern "C"` Rust function with the same ABI.

Each such C function has the uniform shape:

    RET NAME(<params>) {
      ensure_init();
      typedef RET (*pfn)(<typedef-param-types>);
      static pfn fn = NULL;
      if (!fn) fn = (pfn)dlsym(g_backend_lib, "NAME");
      RET _ret = fn ? fn(<args>) : <default>;
      pt_log(2, "...");
      return _ret;            // (omitted for void)
    }

We use the typedef's parameter-type list as the authoritative ABI and pair it
positionally with the argument names from the `fn(<args>)` call. The output is
included by lib.rs via include!(). The table-routed functions (FWD macros and
the manual launch/register/error ones) are emitted by lib.rs itself and are
skipped here to avoid duplicate symbols.
"""
import re
import sys
from pathlib import Path

C_SRC = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(
    "/home/nod/github/hrx-system/libhrx/src/passthrough/hip_intercept.c")
OUT = Path(sys.argv[2]) if len(sys.argv) > 2 else Path(
    __file__).parent / "src" / "passthrough_generated.rs"

# Functions already exported by lib.rs (table-routed). Skip to avoid clashes.
TABLE_ROUTED = {
    # FWD0-FWD5
    "hipInit", "hipDriverGetVersion", "hipRuntimeGetVersion", "hipGetDevice",
    "hipGetDeviceCount", "hipSetDevice", "hipDeviceReset", "hipDeviceSynchronize",
    "hipGetDeviceProperties", "hipDeviceGetAttribute", "hipDeviceGetName",
    "hipMalloc", "hipFree", "hipHostMalloc", "hipHostFree", "hipMemGetInfo",
    "hipMemcpy", "hipMemcpyAsync", "hipMemset", "hipMemsetAsync",
    "hipStreamCreate", "hipStreamCreateWithFlags", "hipStreamDestroy",
    "hipStreamSynchronize", "hipStreamQuery", "hipStreamWaitEvent",
    "hipEventCreate", "hipEventCreateWithFlags", "hipEventDestroy",
    "hipEventRecord", "hipEventSynchronize", "hipEventQuery", "hipEventElapsedTime",
    "hipModuleLoad", "hipModuleLoadData", "hipModuleUnload", "hipModuleGetFunction",
    "hipModuleGetGlobal", "hipGetLastError", "hipPeekAtLastError",
    # manual table-routed
    "hipModuleLaunchKernel", "hipLaunchKernel", "hipExtModuleLaunchKernel",
    "__hipRegisterFatBinary", "__hipUnregisterFatBinary", "__hipRegisterFunction",
    "__hipRegisterVar", "hipGetErrorString", "hipGetErrorName",
}

# C type -> Rust type. Pointers collapse by base; const-ness maps to *const.
SCALARS = {
    "int": "c_int", "unsigned int": "c_uint", "unsigned": "c_uint",
    "size_t": "usize", "uint32_t": "u32", "uint64_t": "u64",
    "unsigned long long": "c_ulonglong", "unsigned long": "c_ulong",
    "unsigned char": "c_uchar", "unsigned short": "c_ushort",
    "float": "c_float", "hipError_t": "hipError_t",
    "hipMemcpyKind": "hipMemcpyKind", "hipDeviceAttribute_t": "hipDeviceAttribute_t",
    "hipMemPoolAttr": "c_uint", "dim3": "dim3",
    # opaque handle typedefs (void* in the header) passed by value
    "hipStream_t": "*mut c_void", "hipEvent_t": "*mut c_void",
    "hipModule_t": "*mut c_void", "hipFunction_t": "*mut c_void",
    "hipDeviceptr_t": "*mut c_void", "hipCtx_t": "*mut c_void",
    "hipDevice_t": "c_int", "hipArray_t": "*mut c_void",
    "hipMipmappedArray_t": "*mut c_void", "hipMemPool_t": "*mut c_void",
    "hipGraph_t": "*mut c_void", "hipGraphExec_t": "*mut c_void",
    "hipGraphNode_t": "*mut c_void", "hipExternalMemory_t": "*mut c_void",
    "hipExternalSemaphore_t": "*mut c_void",
    "hipStreamCaptureStatus": "c_uint",
    # function-pointer params: ABI is a plain pointer
    "hipHostFn_t": "*mut c_void", "hipStreamCallback_t": "*mut c_void",
    # by-value IPC handle structs (char[64]) -> 64-byte repr
    "hipIpcMemHandle_t": "HipIpcHandle", "hipIpcEventHandle_t": "HipIpcHandle",
}
# struct types only ever used behind a pointer -> opaque
OPAQUE_STRUCTS = {
    "hipDeviceProp_t", "hipPointerAttribute_t", "hipMemPoolProps",
}


def c_type_to_rust(t: str) -> str:
    t = t.strip()
    nptr = t.count("*")
    base = t.replace("*", "").strip()
    is_const = base.startswith("const ")
    if is_const:
        base = base[len("const "):].strip()
    if nptr == 0:
        if base in SCALARS:
            return SCALARS[base]
        raise ValueError(f"unmapped scalar type: {t!r}")
    # pointer type
    if base == "void":
        inner = "c_void"
    elif base == "char":
        inner = "c_char"
    elif base in SCALARS and SCALARS[base] not in ("*mut c_void",):
        inner = SCALARS[base]
    elif base in OPAQUE_STRUCTS or base in SCALARS:
        # opaque struct pointer, or pointer-to-handle (e.g. hipStream_t*)
        inner = "c_void" if base in OPAQUE_STRUCTS else SCALARS[base].replace("*mut ", "")
    elif base == "uint32_t":
        inner = "u32"
    else:
        inner = "c_void"
    rust = inner
    for i in range(nptr):
        # innermost qualifier const only for the first level next to base
        if i == 0 and is_const:
            rust = f"*const {rust}"
        else:
            rust = f"*mut {rust}"
    return rust


def split_params(s: str):
    """Split a C parameter type list on top-level commas."""
    s = re.sub(r"\s+", " ", s).strip()
    if s == "void" or s == "":
        return []
    out, depth, cur = [], 0, ""
    for ch in s:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur.strip())
    return out


def split_top_level_functions(src: str):
    """Yield each top-level `{...}` function body as a string, so per-function
    regexes can't greedily span across neighbouring functions."""
    i, n = 0, len(src)
    while i < n:
        brace = src.find("{", i)
        if brace == -1:
            break
        # Walk back to the start of this definition (previous ; } or start).
        start = max(src.rfind(";", 0, brace), src.rfind("}", 0, brace)) + 1
        depth, j = 0, brace
        while j < n:
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        yield src[start:j + 1]
        i = j + 1


def main():
    src = C_SRC.read_text()
    # Process per-function so a non-greedy match cannot span two functions:
    # search each top-level body independently.
    bodies = list(split_top_level_functions(src))
    # Match each direct-passthrough function body. They contain the marker
    # `dlsym(g_backend_lib, "NAME")` and a `typedef RET (*pfn)(PARAMS);` and a
    # `fn(ARGS)` call. Capture the function header to get arg names.
    pat = re.compile(
        r'(?P<ret>hipError_t|const char \*|void \*\*|void)\s*'
        r'(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\((?P<decl>[^;{]*?)\)\s*\{\s*'
        r'ensure_init\(\);\s*'
        r'typedef\s+(?P<tret>[^()]+?)\s*\(\*pfn\)\((?P<tparams>.*?)\);\s*'
        r'static pfn fn = NULL;\s*'
        r'if \(!fn\)\s*fn\s*=\s*\(pfn\)dlsym\(\s*g_backend_lib,\s*"(?P<sym>[^"]+)"\);\s*'
        r'(?P<rettype>hipError_t|const char \*|void \*\*)?\s*_ret\s*=\s*fn\s*\?\s*fn\((?P<args>[^)]*)\)',
        re.S)

    funcs = []
    for body in bodies:
        m = pat.search(body)
        if not m:
            continue
        name = m.group("name")
        if name in TABLE_ROUTED:
            continue
        ret = m.group("ret").strip()
        tparams = split_params(m.group("tparams"))
        args = [a.strip() for a in m.group("args").split(",") if a.strip()]
        if len(args) != len(tparams):
            # signature/arg mismatch -> skip safely (reported)
            funcs.append((name, ret, None, None, "arg/type count mismatch"))
            continue
        try:
            rtypes = [c_type_to_rust(t) for t in tparams]
        except ValueError as e:
            funcs.append((name, ret, None, None, str(e)))
            continue
        funcs.append((name, ret, list(zip(args, rtypes)), tparams, None))

    # void-returning direct functions also exist (e.g. some register helpers);
    # handle ret == "void" with no _ret. Add a second pass for those.
    void_pat = re.compile(
        r'void\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\((?P<decl>[^;{]*?)\)\s*\{\s*'
        r'ensure_init\(\);\s*'
        r'typedef\s+void\s*\(\*pfn\)\((?P<tparams>.*?)\);\s*'
        r'static pfn fn = NULL;\s*'
        r'if \(!fn\)\s*fn\s*=\s*\(pfn\)dlsym\(\s*g_backend_lib,\s*"(?P<sym>[^"]+)"\);\s*'
        r'if \(fn\)\s*fn\((?P<args>[^)]*)\)',
        re.S)
    seen = {f[0] for f in funcs if f[4] is None}
    void_matches = (mm for b in bodies for mm in [void_pat.search(b)] if mm)
    for m in void_matches:
        name = m.group("name")
        if name in TABLE_ROUTED or name in seen:
            continue
        tparams = split_params(m.group("tparams"))
        args = [a.strip() for a in m.group("args").split(",") if a.strip()]
        if len(args) != len(tparams):
            funcs.append((name, "void", None, None, "void arg/type mismatch"))
            continue
        try:
            rtypes = [c_type_to_rust(t) for t in tparams]
        except ValueError as e:
            funcs.append((name, "void", None, None, str(e)))
            continue
        funcs.append((name, "void", list(zip(args, rtypes)), tparams, None))

    ok = [f for f in funcs if f[4] is None]
    ok_names = {f[0] for f in ok}
    # A name that failed one pass but succeeded another is not actually bad.
    bad = [f for f in funcs if f[4] is not None and f[0] not in ok_names]

    lines = [
        "// @generated by generate_passthrough.py from hip_intercept.c — DO NOT EDIT.",
        "// Direct-passthrough HIP exports: dlsym the backend symbol on first use",
        "// and forward to it (bypassing the interceptor table), matching the C",
        "// hip_intercept.c semantics. Default-on-missing matches the C `: 1`/`: NULL`.",
        "#![allow(non_snake_case, unused_imports, clippy::missing_safety_doc)]",
        "use core::ffi::{c_char, c_int, c_uint, c_uchar, c_ushort, c_ulong, c_ulonglong, c_float, c_void};",
        "use hip_function_table::{hipError_t, hipMemcpyKind, hipDeviceAttribute_t, dim3};",
        "use crate::{ensure_init, backend_sym, HipIpcHandle};",
        "",
    ]

    def rust_ret(ret):
        return {"hipError_t": "hipError_t", "const char *": "*const c_char",
                "void **": "*mut *mut c_void", "void": "()"}[ret]

    def default_ret(ret):
        # Matches the C originals' fn-missing defaults: 1 for status,
        # "unknown" for the error/name string getters, NULL for void**.
        return {"hipError_t": "1", "const char *": "c\"unknown\".as_ptr()",
                "void **": "core::ptr::null_mut()", "void": ""}[ret]

    for name, ret, params, tparams, _ in ok:
        rret = rust_ret(ret)
        sig_args = ", ".join(f"{a}: {t}" for a, t in params)
        pfn_args = ", ".join(t for _, t in params)
        call_args = ", ".join(a for a, _ in params)
        arrow = "" if ret == "void" else f" -> {rret}"
        pfn_ty = f"unsafe extern \"C\" fn({pfn_args})" + ("" if ret == "void" else f" -> {rret}")
        lines.append(f'#[no_mangle]')
        lines.append(f'pub unsafe extern "C" fn {name}({sig_args}){arrow} {{')
        lines.append(f'    ensure_init();')
        lines.append(f'    let __p = backend_sym(b"{name}\\0");')
        # NB: use a local name that cannot collide with a HIP parameter (some
        # functions have a parameter literally named `f`).
        if ret == "void":
            lines.append(f'    if !__p.is_null() {{')
            lines.append(f'        let __fn: {pfn_ty} = core::mem::transmute(__p);')
            lines.append(f'        __fn({call_args});')
            lines.append(f'    }}')
        else:
            lines.append(f'    if __p.is_null() {{ return {default_ret(ret)}; }}')
            lines.append(f'    let __fn: {pfn_ty} = core::mem::transmute(__p);')
            lines.append(f'    __fn({call_args})')
        lines.append("}")
        lines.append("")

    OUT.write_text("\n".join(lines))
    print(f"generated {len(ok)} direct-passthrough functions -> {OUT}")
    if bad:
        print(f"SKIPPED {len(bad)} (need manual handling):")
        for name, ret, _, _, why in bad:
            print(f"  {name} ({ret}): {why}")


if __name__ == "__main__":
    main()
