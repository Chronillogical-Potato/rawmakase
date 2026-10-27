# Native regression for #553. Run on an interactive Windows desktop:
# powershell -ExecutionPolicy Bypass -File scripts/test-windows-maximized.ps1 -Exe path\to\rawmakase.exe
# Uses an isolated session and empty catalog; no existing preferences are changed.
# To expose #553, use a work area larger than 1440x960 logical pixels
# (for example, 1920x1080 at 100% scaling). Smaller screens can hide the bug.
param([Parameter(Mandatory = $true)][string]$Exe)
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class MaximizeProbe {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr window);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out RECT rect);
}
'@
$previousData = $env:RAWMAKASE_DATA_DIR
$data = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
$app = $null
function Open-App {
    $script:app = Start-Process $Exe -PassThru
    if (!$app.WaitForInputIdle(30000)) { throw 'App did not become idle within 30 seconds' }
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Seconds 1
        $app.Refresh()
        if ($app.HasExited) { throw "App exited with code $($app.ExitCode)" }
        if ($app.MainWindowHandle -ne 0) {
            Start-Sleep -Seconds 4
            return
        }
    }
    throw 'No window within 60 seconds'
}
function Close-App {
    $app.CloseMainWindow() | Out-Null
    if (!$app.WaitForExit(15000)) { throw 'Close did not finish within 15 seconds' }
}
function Bounds {
    $rect = New-Object MaximizeProbe+RECT
    if (![MaximizeProbe]::GetWindowRect($app.MainWindowHandle, [ref]$rect)) {
        throw 'GetWindowRect failed'
    }
    return "$($rect.Left),$($rect.Top),$($rect.Right),$($rect.Bottom)"
}
function Saved-Maximized {
    return (Get-Content (Join-Path $data 'session.json') -Raw | ConvertFrom-Json).maximized -eq $true
}
try {
    New-Item -ItemType Directory $data | Out-Null
    $env:RAWMAKASE_DATA_DIR = $data
    '{"onboarding_done":true,"no_update_checks":true}' |
        Set-Content (Join-Path $data 'session.json') -Encoding ascii
    Open-App
    [MaximizeProbe]::ShowWindow($app.MainWindowHandle, 3) | Out-Null
    Start-Sleep -Seconds 3
    if (!(Saved-Maximized)) { throw 'Maximizing did not save the state' }
    $maximizedBounds = Bounds
    Close-App
    Open-App
    $reopenedBounds = Bounds
    if (![MaximizeProbe]::IsZoomed($app.MainWindowHandle) -or $reopenedBounds -ne $maximizedBounds) {
        throw "Reopened bounds $reopenedBounds; expected maximized bounds $maximizedBounds (IsZoomed=$([MaximizeProbe]::IsZoomed($app.MainWindowHandle)))"
    }
    [MaximizeProbe]::ShowWindow($app.MainWindowHandle, 6) | Out-Null
    Start-Sleep -Seconds 3
    if (!(Saved-Maximized)) { throw 'Minimizing lost the maximized state' }
    [MaximizeProbe]::ShowWindow($app.MainWindowHandle, 3) | Out-Null
    Start-Sleep -Seconds 1
    [MaximizeProbe]::ShowWindow($app.MainWindowHandle, 9) | Out-Null
    Start-Sleep -Seconds 3
    if (Saved-Maximized) { throw 'Restoring did not save the unmaximized state' }
    Close-App
    Open-App
    if ([MaximizeProbe]::IsZoomed($app.MainWindowHandle)) { throw 'Restored window reopened maximized' }
    Close-App
    Write-Output 'PASS: maximize, reopen with native maximized bounds, minimize, restore, reopen restored'
} finally {
    if ($null -ne $app -and !$app.HasExited) { $app.Kill(); $app.WaitForExit(5000) | Out-Null }
    $env:RAWMAKASE_DATA_DIR = $previousData
    Remove-Item $data -Recurse -Force
}
