# Deterministic, invented UI frames. This script never reads the screen.
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$fixtureDirectory = Join-Path $PSScriptRoot 'fixtures'
[void][System.IO.Directory]::CreateDirectory($fixtureDirectory)

function Get-Color {
    param([string]$Hex)
    return [System.Drawing.ColorTranslator]::FromHtml($Hex)
}

function Draw-Box {
    param($Graphics, [int]$X, [int]$Y, [int]$Width, [int]$Height,
        [string]$Fill, [string]$Border = '')
    $brush = [System.Drawing.SolidBrush]::new((Get-Color $Fill))
    try {
        $Graphics.FillRectangle($brush, $X, $Y, $Width, $Height)
    } finally {
        $brush.Dispose()
    }
    if ($Border) {
        $pen = [System.Drawing.Pen]::new((Get-Color $Border), 1)
        try {
            $Graphics.DrawRectangle($pen, $X, $Y, $Width, $Height)
        } finally {
            $pen.Dispose()
        }
    }
}

function Draw-Text {
    param($Graphics, [string]$Text, [int]$X, [int]$Y,
        [int]$Size = 20, [string]$Color = '#17202A', [switch]$Bold,
        [switch]$Monospace)
    $family = if ($Monospace) { 'Consolas' } else { 'Segoe UI' }
    $style = if ($Bold) { [System.Drawing.FontStyle]::Bold } else { [System.Drawing.FontStyle]::Regular }
    $font = [System.Drawing.Font]::new($family, $Size, $style, [System.Drawing.GraphicsUnit]::Pixel)
    $brush = $null
    try {
        $brush = [System.Drawing.SolidBrush]::new((Get-Color $Color))
        $Graphics.DrawString($Text, $font, $brush, [single]$X, [single]$Y)
    } finally {
        if ($null -ne $brush) { $brush.Dispose() }
        $font.Dispose()
    }
}

function Draw-Line {
    param($Graphics, [int]$X1, [int]$Y1, [int]$X2, [int]$Y2,
        [string]$Color, [int]$Width = 1)
    $pen = [System.Drawing.Pen]::new((Get-Color $Color), $Width)
    try {
        $Graphics.DrawLine($pen, $X1, $Y1, $X2, $Y2)
    } finally {
        $pen.Dispose()
    }
}

function New-Fixture {
    param([string]$Id, [string]$File, [int]$Width, [int]$Height,
        [string[]]$GroundTruth, [scriptblock]$Draw)
    $bitmap = [System.Drawing.Bitmap]::new($Width, $Height)
    $graphics = $null
    $path = Join-Path $fixtureDirectory $File
    try {
        $bitmap.SetResolution(96, 96)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        & $Draw $graphics
        $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally {
        if ($null -ne $graphics) { $graphics.Dispose() }
        $bitmap.Dispose()
    }
    [pscustomobject]@{
        id = $Id
        file = $File
        width = $Width
        height = $Height
        sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        ground_truth = $GroundTruth
    }
}

$manifest = @(
    New-Fixture -Id 'dialog' -File '01-dialog.png' -Width 640 -Height 360 -GroundTruth @(
        'The image is labeled SYNTHETIC TEST UI.'
        'A centered dialog is titled Upload failed.'
        'The dialog says Network unavailable.'
        'The dialog contains Retry and Cancel buttons.'
    ) -Draw {
        param($g)
        $g.Clear((Get-Color '#E8EDF2'))
        Draw-Box $g 0 0 640 48 '#263544'
        Draw-Text $g 'SYNTHETIC TEST UI' 18 10 20 '#FFFFFF' -Bold
        Draw-Box $g 64 76 512 230 '#FFFFFF' '#A9B5C0'
        Draw-Box $g 64 76 512 6 '#C43232'
        Draw-Text $g 'Upload failed' 92 100 28 '#A52323' -Bold
        Draw-Text $g 'Network unavailable' 92 154 24
        Draw-Text $g 'Check the connection, then try again.' 92 191 20 '#455564'
        Draw-Box $g 298 248 112 40 '#245AA5'
        Draw-Text $g 'Retry' 328 253 22 '#FFFFFF' -Bold
        Draw-Box $g 426 248 122 40 '#EDF1F5' '#8394A3'
        Draw-Text $g 'Cancel' 451 253 22
    }
    New-Fixture -Id 'form' -File '02-form.png' -Width 960 -Height 640 -GroundTruth @(
        'The image is labeled SYNTHETIC TEST UI.'
        'A Create account signup form is visible.'
        'The Email field contains demo.invalid.'
        'Red validation text says Invalid email address.'
        'The Password field is masked with asterisks.'
        'The Create account button is gray and looks disabled.'
    ) -Draw {
        param($g)
        $g.Clear((Get-Color '#EEF2F6'))
        Draw-Box $g 0 0 960 56 '#263544'
        Draw-Text $g 'SYNTHETIC TEST UI - Example signup' 24 14 22 '#FFFFFF' -Bold
        Draw-Box $g 190 90 580 510 '#FFFFFF' '#A9B5C0'
        Draw-Text $g 'Create account' 230 119 28 '#17202A' -Bold
        Draw-Text $g 'Use an email address to sign up.' 230 164 20 '#455564'
        Draw-Text $g 'Email' 230 215 22 '#17202A' -Bold
        Draw-Box $g 230 250 500 52 '#FFFAFA' '#C43232'
        Draw-Text $g 'demo.invalid' 246 262 24
        Draw-Text $g 'Invalid email address' 230 313 22 '#BC2525' -Bold
        Draw-Text $g 'Password' 230 366 22 '#17202A' -Bold
        Draw-Box $g 230 402 500 52 '#FFFFFF' '#8394A3'
        Draw-Text $g '************' 246 414 24 '#17202A' -Monospace
        Draw-Box $g 230 499 500 54 '#D9DEE4' '#B1BBC5'
        Draw-Text $g 'Create account' 394 511 24 '#74808C' -Bold
    }
    New-Fixture -Id 'workspace' -File '03-workspace.png' -Width 1280 -Height 800 -GroundTruth @(
        'The image is labeled SYNTHETIC TEST UI.'
        'An invented build workspace has sidebar entries Files, Builds, and Settings.'
        'A file list includes main.rs, ui.rs, and config.rs.'
        'A blue line chart is titled Build duration (seconds).'
        'A build log says error E0425: cannot find value total.'
        'The build status says Build failed.'
    ) -Draw {
        param($g)
        $g.Clear((Get-Color '#F0F3F7'))
        Draw-Box $g 0 0 1280 58 '#263544'
        Draw-Text $g 'SYNTHETIC TEST UI - Invented build workspace' 24 15 24 '#FFFFFF' -Bold
        Draw-Box $g 0 58 204 742 '#E0E7EE'
        Draw-Text $g 'Workspace' 22 88 24 '#263544' -Bold
        Draw-Text $g 'Files' 26 152 22
        Draw-Box $g 12 194 178 48 '#C8D9EC'
        Draw-Text $g 'Builds' 26 204 22 '#184C8C' -Bold
        Draw-Text $g 'Settings' 26 262 22
        Draw-Box $g 228 82 320 324 '#FFFFFF' '#B7C2CD'
        Draw-Text $g 'Project files' 250 102 24 '#17202A' -Bold
        Draw-Line $g 248 146 528 146 '#D1D9E1'
        Draw-Text $g 'main.rs' 252 172 22 '#264D76' -Monospace
        Draw-Text $g 'ui.rs' 252 224 22 '#264D76' -Monospace
        Draw-Text $g 'config.rs' 252 276 22 '#264D76' -Monospace
        Draw-Text $g '3 source files' 252 351 18 '#556575'
        Draw-Box $g 572 82 684 324 '#FFFFFF' '#B7C2CD'
        Draw-Text $g 'Build duration (seconds)' 594 102 24 '#17202A' -Bold
        Draw-Line $g 642 166 642 346 '#657585'
        Draw-Line $g 642 346 1220 346 '#657585'
        foreach ($tick in @(0, 10, 20)) {
            $y = 346 - ($tick * 8)
            Draw-Line $g 642 $y 1220 $y '#E1E6EC'
            Draw-Text $g ([string]$tick) 598 ($y - 12) 18 '#556575'
        }
        $points = @(@(670, 286), @(770, 250), @(870, 270), @(970, 214), @(1070, 238), @(1170, 182))
        for ($i = 1; $i -lt $points.Count; $i++) {
            Draw-Line $g $points[$i - 1][0] $points[$i - 1][1] $points[$i][0] $points[$i][1] '#246EC3' 4
        }
        Draw-Text $g 'Run 1' 650 360 18 '#556575'
        Draw-Text $g 'Run 6' 1150 360 18 '#556575'
        Draw-Box $g 228 430 1028 338 '#18232E'
        Draw-Text $g 'Build log' 250 449 24 '#FFFFFF' -Bold
        Draw-Line $g 250 492 1234 492 '#4A5866'
        Draw-Text $g '> cargo build' 250 509 20 '#DCE6F0' -Monospace
        Draw-Text $g 'Compiling example_ui v0.1.0' 250 549 20 '#DCE6F0' -Monospace
        Draw-Text $g 'error E0425: cannot find value total' 250 589 22 '#FF9393' -Monospace
        Draw-Text $g '  --> src/main.rs:12:20' 250 629 20 '#DCE6F0' -Monospace
        Draw-Text $g 'Build failed' 250 696 26 '#FF9393' -Bold
    }
)

$json = (ConvertTo-Json -InputObject $manifest -Depth 4).Replace("`r`n", "`n")
$manifestPath = Join-Path $fixtureDirectory 'manifest.json'
[System.IO.File]::WriteAllText($manifestPath, $json + "`n", [System.Text.UTF8Encoding]::new($false))
Write-Output ('Generated {0} synthetic fixtures in {1}' -f $manifest.Count, $fixtureDirectory)
