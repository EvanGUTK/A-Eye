# Drives the actual bundled WebView2 UI, without a browser debugging port or web server.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Screenshot,
    [switch]$KeepRunning
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class DesktopVerifyWindow {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
[void][DesktopVerifyWindow]::SetProcessDPIAware()

function Get-Elements {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($script:app.MainWindowHandle)
    $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
}

function Wait-Text([string]$Pattern, [int]$TimeoutSec = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSec)
    do {
        foreach ($element in (Get-Elements)) {
            if ($element.Current.Name -match $Pattern) { return $element }
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "UI text did not appear: $Pattern"
}

function Invoke-Button([string]$Name) {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        foreach ($element in (Get-Elements)) {
            if ($element.Current.Name -eq $Name -and $element.Current.IsEnabled -and
                $element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button) {
                $pattern = $element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
                $pattern.Invoke()
                return
            }
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Enabled button did not appear: $Name"
}

function Get-Tree {
    $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CreationDate)
    $found = [System.Collections.Generic.HashSet[int]]::new()
    [void]$found.Add($script:app.Id)
    $queue = [System.Collections.Generic.Queue[object]]::new()
    $queue.Enqueue(($all | Where-Object { $_.ProcessId -eq $script:app.Id }))
    while ($queue.Count -gt 0) {
        $parent = $queue.Dequeue()
        foreach ($child in $all) {
            if ($child.ParentProcessId -eq $parent.ProcessId -and $child.CreationDate -ge $parent.CreationDate -and $found.Add([int]$child.ProcessId)) {
                $queue.Enqueue($child)
            }
        }
    }
    $all | Where-Object { $found.Contains([int]$_.ProcessId) }
}

function Get-Sidecar {
    $children = @(Get-Tree | Where-Object { $_.ParentProcessId -eq $script:app.Id -and $_.Name -eq 'aeye-mcp.exe' })
    if ($children.Count -ne 1) { throw "Expected one direct MCP child; found $($children.Count)" }
    $children[0]
}

$script:app = Start-Process -FilePath $Exe -PassThru
$passed = $false
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        $script:app.Refresh()
        if ($script:app.HasExited) { throw 'Desktop exited before its window appeared.' }
        if ($script:app.MainWindowHandle -ne 0) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($script:app.MainWindowHandle -eq 0) { throw 'Desktop window missing.' }
    [void][DesktopVerifyWindow]::SetForegroundWindow($script:app.MainWindowHandle)
    [void](Wait-Text '^Connected')
    $sidecar = Get-Sidecar
    Write-Host 'PASS real window, MCP handshake, live display list, direct stdio child'

    Invoke-Button 'Show orange overlay'
    [void](Wait-Text '^Overlay requested')
    Invoke-Button 'Capture one frame'
    $decoded = Wait-Text '^Preview decoded: [0-9]+'
    Write-Host "PASS UI capture and decoded image: $($decoded.Current.Name)"

    $tree = @(Get-Tree)
    $treeIds = @($tree | ForEach-Object { [int]$_.ProcessId })
    $tcp = @(Get-NetTCPConnection -ErrorAction Stop | Where-Object { $treeIds -contains $_.OwningProcess })
    $udp = @(Get-NetUDPEndpoint -ErrorAction Stop | Where-Object { $treeIds -contains $_.OwningProcess })
    if ($tcp.Count -ne 0) { throw "App process tree owns $($tcp.Count) TCP endpoint(s)." }
    Write-Host "PASS no TCP endpoints in app/sidecar/WebView2 tree ($($tree.Count) processes); UDP endpoints=$($udp.Count) (reported only)"

    if ($Screenshot) {
        $rect = New-Object DesktopVerifyWindow+RECT
        [void][DesktopVerifyWindow]::GetWindowRect($script:app.MainWindowHandle, [ref]$rect)
        $bitmap = New-Object System.Drawing.Bitmap ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
            $bitmap.Save($Screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
        } finally { $graphics.Dispose(); $bitmap.Dispose() }
        Write-Host 'Saved app-window evidence locally (may contain private preview pixels; do not commit).'
    }
    Invoke-Button 'Hide overlay'
    [void](Wait-Text '^No overlay requested')
    Invoke-Button 'Disconnect'
    [void](Wait-Text '^Not connected')
    if (Get-Process -Id $sidecar.ProcessId -ErrorAction SilentlyContinue) { throw 'Sidecar survived Disconnect.' }
    Write-Host 'PASS hide overlay and disconnect child cleanup'
    Invoke-Button 'Connect'
    [void](Wait-Text '^Connected')
    $sidecar = Get-Sidecar
    Stop-Process -Id $sidecar.ProcessId -Force
    [void](Wait-Text '^Not connected')
    Invoke-Button 'Connect'
    [void](Wait-Text '^Connected')
    Write-Host 'PASS child crash is shown as disconnected; explicit reconnect works'

    Invoke-Button 'Show orange overlay'
    Invoke-Button 'Hide overlay'
    [void](Wait-Text '^No overlay requested')
    Invoke-Button 'Disconnect'
    [void](Wait-Text '^Not connected')
    Invoke-Button 'Connect'
    Invoke-Button 'Disconnect'
    [void](Wait-Text '^Not connected')
    if (@(Get-Tree | Where-Object { $_.Name -eq 'aeye-mcp.exe' }).Count -ne 0) { throw 'Child appeared after Connect/Disconnect.' }
    Invoke-Button 'Connect'
    [void](Wait-Text '^Connected')
    Write-Host 'PASS immediate show/hide and connect/disconnect preserve final stopped state'

    Invoke-Button 'Show orange overlay'
    [void](Wait-Text '^Overlay requested')
    $sidecar = Get-Sidecar
    [void]$script:app.CloseMainWindow()
    if (-not $script:app.WaitForExit(10000)) { throw 'Desktop did not close in ten seconds.' }
    if (Get-Process -Id $sidecar.ProcessId -ErrorAction SilentlyContinue) { throw 'Sidecar survived window close.' }
    Write-Host 'PASS closing the actual window stops the child with an overlay requested'
    $passed = $true
} finally {
    if (-not $script:app.HasExited) {
        [void]$script:app.CloseMainWindow()
        if (-not $script:app.WaitForExit(15000)) { Stop-Process -Id $script:app.Id -Force }
    }
}
if ($passed -and $KeepRunning) {
    $demo = Start-Process -FilePath $Exe -PassThru
    Write-Host "Demo left running: PID=$($demo.Id); stop by closing its window."
}
