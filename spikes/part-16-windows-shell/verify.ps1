# Spike 16 verification: launch the Tauri exe, prove it owns a real window, and prove it opens no
# network listener (app process plus every descendant, e.g. msedgewebview2.exe).
# Usage: powershell -File verify.ps1 -Exe <path to exe> [-Screenshot <png path>]
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Screenshot,
    [int]$WindowTimeoutSec = 20,
    [int]$SettleSec = 5
)
$ErrorActionPreference = 'Stop'

function Get-Descendants([int]$RootPid) {
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CreationDate
    $created = @{}
    foreach ($p in $all) { $created[[int]$p.ProcessId] = $p.CreationDate }
    $found = [System.Collections.Generic.List[object]]::new()
    $queue = [System.Collections.Generic.Queue[int]]::new()
    $queue.Enqueue($RootPid)
    while ($queue.Count -gt 0) {
        $parent = $queue.Dequeue()
        # A child cannot predate its parent; ParentProcessId may name a dead process whose PID was reused.
        foreach ($p in $all | Where-Object { $_.ParentProcessId -eq $parent -and $_.ProcessId -ne $parent -and $_.CreationDate -ge $created[$parent] }) {
            $found.Add($p)
            $queue.Enqueue([int]$p.ProcessId)
        }
    }
    $found
}

function Get-ListeningPorts {
    Get-NetTCPConnection -State Listen | ForEach-Object { "$($_.LocalAddress):$($_.LocalPort)" } | Sort-Object -Unique
}

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class WinApi {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
[void][WinApi]::SetProcessDPIAware()

$before = Get-ListeningPorts
$proc = Start-Process -FilePath $Exe -PassThru
try {
    Write-Host "launched pid=$($proc.Id) exe=$Exe"

    $deadline = (Get-Date).AddSeconds($WindowTimeoutSec)
    do {
        Start-Sleep -Milliseconds 250
        $proc.Refresh()
    } while ($proc.MainWindowHandle -eq 0 -and -not $proc.HasExited -and (Get-Date) -lt $deadline)

    if ($proc.HasExited) { Write-Host "FAIL: process exited early, code=$($proc.ExitCode)"; exit 1 }
    $hwnd = $proc.MainWindowHandle
    Write-Host ("MainWindowHandle=0x{0:X}  MainWindowTitle='{1}'  visible={2}" -f [int64]$hwnd, $proc.MainWindowTitle, [WinApi]::IsWindowVisible($hwnd))
    $windowOk = ($hwnd -ne 0) -and $proc.MainWindowTitle -and [WinApi]::IsWindowVisible($hwnd)

    Start-Sleep -Seconds $SettleSec   # let WebView2 finish spawning its helper processes
    $proc.Refresh()
    $stillRunning = -not $proc.HasExited
    $windowOk = $windowOk -and $stillRunning -and [WinApi]::IsWindowVisible($hwnd)
    Write-Host "after settle: running=$stillRunning window_visible=$([WinApi]::IsWindowVisible($hwnd))"
    $descendants = @(Get-Descendants $proc.Id)
    $pids = @($proc.Id) + @($descendants | ForEach-Object { [int]$_.ProcessId })
    $byName = ($descendants | Group-Object Name | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ', '
    Write-Host "process tree: app + $($descendants.Count) descendants: $byName"
    Write-Host "tree pids: $($pids -join ',')"

    $tcp = @(Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object { $pids -contains $_.OwningProcess })
    $udp = @(Get-NetUDPEndpoint -ErrorAction SilentlyContinue | Where-Object { $pids -contains $_.OwningProcess })
    Write-Host "TCP sockets owned by tree: $($tcp.Count)"
    $tcp | ForEach-Object { Write-Host "  TCP $($_.State) $($_.LocalAddress):$($_.LocalPort) -> $($_.RemoteAddress):$($_.RemotePort) pid=$($_.OwningProcess)" }
    Write-Host "UDP endpoints owned by tree: $($udp.Count)"
    $udp | ForEach-Object {
        $role = (Get-CimInstance Win32_Process -Filter "ProcessId=$($_.OwningProcess)").CommandLine -replace '^.*--utility-sub-type=(\S+).*$', '$1'
        Write-Host "  UDP $($_.LocalAddress):$($_.LocalPort) pid=$($_.OwningProcess) role=$role"
    }

    $after = Get-ListeningPorts
    $newListeners = @($after | Where-Object { $before -notcontains $_ })
    Write-Host "new system-wide TCP listeners since before launch: $($newListeners.Count) $($newListeners -join ', ')"

    if ($Screenshot) {
        [void][WinApi]::SetForegroundWindow($hwnd)
        Start-Sleep -Milliseconds 800
        $r = New-Object WinApi+RECT
        [void][WinApi]::GetWindowRect($hwnd, [ref]$r)
        $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
        $bmp = New-Object System.Drawing.Bitmap $w, $h
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
        $bmp.Save($Screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
        Write-Host "screenshot ${w}x${h} -> $Screenshot"
    }
} finally {
    # Stop the whole tree (also on failure) so no orphaned WebView2 helpers are left behind.
    Get-Descendants $proc.Id | Sort-Object ProcessId -Descending | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

# The gate is TCP: a browser-reachable local web server needs a TCP listener. UDP endpoints are
# reported for the record (WebView2's own network service owns one) but do not gate the result.
$netOk = ($tcp.Count -eq 0) -and ($newListeners.Count -eq 0)
Write-Host ("RESULT window={0} no_tcp_endpoints={1} udp_endpoints={2} (reported, not gated)" -f $(if ($windowOk) { 'PASS' } else { 'FAIL' }), $(if ($netOk) { 'PASS' } else { 'FAIL' }), $udp.Count)
if (-not ($windowOk -and $netOk)) { exit 1 }

