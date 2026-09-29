# Stages the Windows payload: the executable, the Microsoft C++ and OpenMP
# runtime DLLs it needs (app-local, so no Visual C++ Redistributable install
# is needed), and the licenses.
#
#   pwsh packaging/windows/stage.ps1 target\release\rawmakase.exe C:\path\to\deps dist\windows
#
# Every DLL the payload imports must either ship in it or be part of Windows;
# anything else fails the build.
param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$Deps,
    [Parameter(Mandatory)][string]$Output
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')

if (Test-Path $Output) { throw "$Output already exists" }
New-Item -ItemType Directory $Output | Out-Null
Copy-Item $Executable (Join-Path $Output 'rawmakase.exe')
Copy-Item (Join-Path $root 'LICENSE'), (Join-Path $root 'README.md') $Output
$licenses = New-Item -ItemType Directory (Join-Path $Output 'licenses')
Copy-Item (Join-Path $root 'licenses\*') $licenses
Copy-Item (Join-Path $Deps 'notices\*') $licenses

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio with the C++ tools is not installed' }
$newest = { Get-ChildItem $args[0] -Directory | Where-Object Name -Match '^\d+(\.\d+)+$' | Sort-Object { [version]$_.Name } | Select-Object -Last 1 }
$tools = & $newest (Join-Path $vs 'VC\Tools\MSVC')
$dumpbin = Join-Path $tools.FullName 'bin\Hostx64\x64\dumpbin.exe'
$redist = & $newest (Join-Path $vs 'VC\Redist\MSVC')
# Microsoft.VC143.CRT and Microsoft.VC143.OpenMP, or their successors.
$runtime = @{}
Get-ChildItem (Join-Path $redist.FullName 'x64') -Directory |
    Where-Object Name -Match '^Microsoft\.VC\d+\.(CRT|OpenMP)$' |
    Get-ChildItem -Filter *.dll | ForEach-Object { $runtime[$_.Name.ToLowerInvariant()] = $_.FullName }
if (-not $runtime['vcomp140.dll']) { throw "No OpenMP runtime under $($redist.FullName)" }

function Get-Imports([string]$path) {
    $inside = $false
    foreach ($line in & $dumpbin /nologo /dependents $path) {
        if ($line -match 'Image has the following dependencies') { $inside = $true; continue }
        if ($inside -and $line -match '^\s+(\S+\.dll)\s*$') { $Matches[1].ToLowerInvariant() }
        elseif ($inside -and $line -match '^\s+Summary') { break }
    }
}

$system = Join-Path $env:SystemRoot 'System32'
$pending = [System.Collections.Generic.Queue[string]]::new()
$pending.Enqueue((Join-Path $Output 'rawmakase.exe'))
$seen = @{}
while ($pending.Count) {
    foreach ($dll in Get-Imports $pending.Dequeue()) {
        if ($seen[$dll]) { continue }
        $seen[$dll] = $true
        $local = Join-Path $Output $dll
        if ($runtime[$dll]) {
            Copy-Item $runtime[$dll] $local
            $pending.Enqueue($local)
        } elseif ($dll -like 'api-ms-win-*' -or $dll -like 'ext-ms-*' -or (Test-Path (Join-Path $system $dll))) {
            # Part of Windows 10 and later, including the Universal C Runtime.
        } else {
            throw "rawmakase.exe needs $dll, which is neither shipped nor part of Windows"
        }
    }
}
Get-ChildItem $Output -Recurse -File | ForEach-Object { $_.FullName.Substring((Resolve-Path $Output).Path.Length + 1) }
