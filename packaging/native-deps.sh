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
fetch LibRaw-0.22.2.tar.gz https://www.libraw.org/data/LibRaw-0.22.2.tar.gz de86b035655accff8d4010f1a221fdf50d353cb7b1422ba26f14a0db92612cfa
jobs=$(getconf _NPROCESSORS_ONLN)
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
# Little CMS exposes a compatibility switch for C++17's removed register keyword.
export CXXFLAGS="${CXXFLAGS:--O2} -DCMS_NO_REGISTER_KEYWORD"
cd "$work/lcms2-2.19.1"
./configure --prefix="$prefix" --disable-static --without-jpeg --without-tiff
make -j"$jobs"
make install
cp LICENSE "$prefix/notices/lcms2-LICENSE"
cd "$work/LibRaw-0.22.2"
# The application's C++ wrapper uses OpenMP; LibRaw itself need not do so.
# JPEG and zlib retain support for compressed DNG files.
./configure --prefix="$prefix" --disable-static --disable-examples --disable-openmp --enable-jpeg --enable-zlib --enable-lcms
for feature in USE_JPEG USE_ZLIB USE_LCMS2; do
  grep -q -- "-D$feature" Makefile || { echo "LibRaw configured without $feature" >&2; exit 1; }
done
make -j"$jobs"
make install
cp COPYRIGHT LICENSE.LGPL LICENSE.CDDL "$prefix/notices/"
