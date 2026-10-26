# Prepares this Windows for the MSVC build. RAWmakase links no C or C++
# library of its own (the catalog's SQLite is compiled by its crate); on ARM64,
# ring assembles with clang, so this puts LLVM's clang on PATH (and
# $GITHUB_PATH when run in GitHub Actions). Nothing to do on x64.
#
#   packaging/windows/toolchain.ps1
$ErrorActionPreference = 'Stop'

if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64' -and
        -not (Get-Command clang -ErrorAction SilentlyContinue)) {
    $llvm = Join-Path $env:ProgramFiles 'LLVM\bin'
    if (-not (Test-Path (Join-Path $llvm 'clang.exe'))) { throw 'LLVM (clang) is not installed' }
    $env:PATH = "$llvm;$env:PATH"
    if ($env:GITHUB_PATH) { $llvm | Out-File -Append -Encoding utf8 $env:GITHUB_PATH }
}
