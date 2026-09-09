#!/usr/bin/env bash
#
# install-cuda-runtime.sh - make FunGen's NVIDIA GPU acceleration work
# WITHOUT installing a system-wide CUDA toolkit.
#
# FunGen's ONNX Runtime CUDA provider dlopens the CUDA 12 + cuDNN 9 runtime
# libraries (libcublasLt.so.12, libcublas.so.12, libcudart.so.12,
# libcufft.so.11, libcurand.so.10, libcudnn.so.9, ...). If they aren't on
# your system, AI tracking/generation silently runs on CPU. Rather than make
# you install a full CUDA toolkit, this script downloads NVIDIA's own
# redistributable runtime wheels from PyPI and drops just the .so files into
# ./lib, beside FunGen's bundled libraries - which already carry an rpath
# that finds them.
#
# It does NOT change your NVIDIA driver, install a CUDA toolkit, add a
# package repository, or touch anything system-wide. Everything lands inside
# this FunGen folder. To undo: delete the libcu*/libcudnn*/libnv* files from
# ./lib, or just delete the whole FunGen folder.
#
# Requirements: python3 (standard library only) + network access, and a
# recent NVIDIA driver already installed (this script does not touch it).

set -euo pipefail

HERE="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
LIBDIR="$HERE/lib"

if [[ ! -d "$LIBDIR" ]]; then
    echo "error: $LIBDIR not found - run this from inside the FunGen folder." >&2
    exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 is required (standard library only). Install it and retry." >&2
    exit 1
fi

case "$(uname -m)" in
    x86_64)  ARCHTOK=x86_64 ;;
    aarch64) ARCHTOK=aarch64 ;;
    *) echo "error: unsupported arch '$(uname -m)' (need x86_64 or aarch64)." >&2; exit 1 ;;
esac

echo "==> Fetching NVIDIA CUDA 12 + cuDNN 9 runtime libraries into ./lib"
echo "    (NVIDIA redistributable wheels from PyPI; nothing installed system-wide)"
echo

LIBDIR="$LIBDIR" ARCHTOK="$ARCHTOK" python3 - <<'PY'
import json, os, shutil, sys, tempfile, urllib.request, zipfile

libdir  = os.environ["LIBDIR"]
archtok = os.environ["ARCHTOK"]

# The runtime sonames ONNX Runtime's CUDA 12 provider needs, mapped to
# NVIDIA's redistributable PyPI wheels. cuDNN ships several sub-libraries
# (ops / engines / heuristic / ...), so we extract every .so* a wheel carries.
packages = [
    "nvidia-cuda-runtime-cu12",   # libcudart.so.12
    "nvidia-cublas-cu12",         # libcublas.so.12, libcublasLt.so.12
    "nvidia-cufft-cu12",          # libcufft.so.11
    "nvidia-curand-cu12",         # libcurand.so.10
    "nvidia-cudnn-cu12",          # libcudnn.so.9 (+ sub-libs)
]

def ver_key(v):
    out = []
    for p in v.split("."):
        out.append(int(p) if p.isdigit() else -1)
    return out

def newest_wheel(pkg):
    with urllib.request.urlopen(f"https://pypi.org/pypi/{pkg}/json", timeout=60) as r:
        meta = json.load(r)
    for ver in sorted(meta["releases"], key=ver_key, reverse=True):
        for f in meta["releases"][ver]:
            fn = f["filename"]
            if (fn.endswith(".whl") and "manylinux" in fn and archtok in fn
                    and not f.get("yanked")):
                return ver, f["url"]
    raise SystemExit(f"error: no manylinux {archtok} wheel found for {pkg}")

extracted = []
with tempfile.TemporaryDirectory() as tmp:
    for pkg in packages:
        ver, url = newest_wheel(pkg)
        print(f"    {pkg} {ver}")
        whl = os.path.join(tmp, os.path.basename(url))
        urllib.request.urlretrieve(url, whl)
        with zipfile.ZipFile(whl) as z:
            for name in z.namelist():
                base = os.path.basename(name)
                # NVIDIA wheels keep the libraries under nvidia/<comp>/lib/.
                if "/lib/" in name and ".so" in base:
                    dest = os.path.join(libdir, base)
                    with z.open(name) as src, open(dest, "wb") as out:
                        shutil.copyfileobj(src, out)
                    os.chmod(dest, 0o755)
                    extracted.append(base)

if not extracted:
    raise SystemExit("error: no .so files extracted - aborting.")

print(f"\n==> Installed {len(set(extracted))} libraries into ./lib:")
for b in sorted(set(extracted)):
    print(f"      {b}")
PY

echo
echo "==> Done. Launch FunGen and run an AI task; the 'FunGen AI' status pill"
echo "    should now read CUDA instead of CPU. If it still says CPU, check that"
echo "    your NVIDIA driver is installed and recent."
