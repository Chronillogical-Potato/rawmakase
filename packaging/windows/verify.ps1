# Installs the finished setup program silently the way the updater runs it,
# checks the installed app reports the release version, then uninstalls it.
#
#   pwsh packaging/windows/verify.ps1 dist\rawmakase-v0.1.9-x86_64-pc-windows-msvc-setup.exe 0.1.9
param(
    [Parameter(Mandatory)][string]$Setup,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
$directory = Join-Path ([System.IO.Path]::GetTempPath()) "rawmakase-verify-$PID"
$log = "$directory.log"

$install = Start-Process -FilePath $Setup -Wait -PassThru -ArgumentList @(
    '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=$directory", "/LOG=$log")
if ($install.ExitCode -ne 0) { Get-Content $log; throw "Setup exited with $($install.ExitCode)" }

$exe = Join-Path $directory 'rawmakase.exe'
foreach ($file in $exe, (Join-Path $directory 'LICENSE'), (Join-Path $directory 'onnxruntime.dll')) {
    if (-not (Test-Path $file)) { throw "Setup did not install $file" }
}
if ((Get-Content (Join-Path $directory 'rawmakase-installer.txt')).Trim() -ne 'rawmakase-installer-v1') {
    throw 'Setup did not write the installer marker'
}
# A windows-subsystem program still writes to redirected output.
$reported = (& $exe --version | Out-String).Trim()
if ($reported -ne "rawmakase $Version") { throw "Installed app reports '$reported', expected 'rawmakase $Version'" }

$uninstaller = Join-Path $directory 'unins000.exe'
$uninstall = Start-Process -FilePath $uninstaller -Wait -PassThru -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
if ($uninstall.ExitCode -ne 0) { throw "Uninstall exited with $($uninstall.ExitCode)" }
# The uninstaller removes its own copy after it exits.
for ($i = 0; $i -lt 30 -and (Test-Path $exe); $i++) { Start-Sleep -Seconds 1 }
if (Test-Path $exe) { throw 'Uninstall left the app behind' }
"Installed, reported '$reported' and uninstalled cleanly"
