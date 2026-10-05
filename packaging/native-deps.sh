#!/usr/bin/env bash
# Build the same imaging libraries on both release platforms. No system install.
set -euo pipefail
prefix="${1:?usage: native-deps.sh ABSOLUTE_PREFIX}"
[[ "$prefix" = /* ]] || exit 1
mkdir -p "$prefix" "$prefix/sources" "$prefix/notices"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fetch() {
  local name="$1" url="$2" expected="$3"
  if [[ ! -f "$prefix/sources/$name" ]]; then
    curl --fail --location --retry 3 --connect-timeout 30 --max-time 300 "$url" -o "$prefix/sources/$name"
  fi
  python3 - "$prefix/sources/$name" "$expected" <<'PY'
import hashlib, pathlib, sys
assert hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() == sys.argv[2], 'Source checksum mismatch'
PY
  tar -xf "$prefix/sources/$name" -C "$work"
}
fetch lcms2-2.19.1.tar.gz https://github.com/mm2/Little-CMS/releases/download/lcms2.19.1/lcms2-2.19.1.tar.gz bfc54f7bab59fbc921012014a8032e4cba4abd46db47d46b76416a8c0b2815c8
# LibRaw from its master branch: the Sony A7 V (ILCE-7M5) and the ARW6 decoder
# its compressed files need landed after 0.22.2, and there is no release with
# them yet. Master carries every fix in 0.22.2. Git archives have no configure
# script, so it is generated below.
libraw=4f0144031c4adfadf7e053937826c49ed80cfa89
fetch "LibRaw-$libraw.tar.gz" "https://github.com/LibRaw/LibRaw/archive/$libraw.tar.gz" f3c70df0f3e2638e5a84fb3b31c580a26e8ad395242589b063787225e0e41510
jobs=$(getconf _NPROCESSORS_ONLN)
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
# Little CMS exposes a compatibility switch for C++17's removed register keyword.
export CXXFLAGS="${CXXFLAGS:--O2} -DCMS_NO_REGISTER_KEYWORD"
cd "$work/lcms2-2.19.1"
./configure --prefix="$prefix" --disable-static --without-jpeg --without-tiff
make -j"$jobs"
make install
cp LICENSE "$prefix/notices/lcms2-LICENSE"
cd "$work/LibRaw-$libraw"
# Homebrew installs GNU libtoolize as glibtoolize, beside Apple's own libtool.
if ! command -v libtoolize >/dev/null && command -v glibtoolize >/dev/null; then
  export LIBTOOLIZE=glibtoolize
fi
autoreconf --install
# LibRaw still unpacks the sensor data before RAWmakase's own demosaic, and its
# decoders for compressed files (Fujifilm RAF, Canon CR3) are OpenMP-parallel.
# JPEG and zlib retain support for compressed DNG files.
./configure --prefix="$prefix" --disable-static --disable-examples --enable-openmp --enable-jpeg --enable-zlib --enable-lcms
for feature in USE_JPEG USE_ZLIB USE_LCMS2; do
  grep -q -- "-D$feature" Makefile || { echo "LibRaw configured without $feature" >&2; exit 1; }
done
make -j"$jobs"
make install
cp COPYRIGHT LICENSE.LGPL LICENSE.CDDL "$prefix/notices/"
