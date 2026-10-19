param([switch]$NoBundle)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows -and $PSVersionTable.PSEdition -eq 'Core') { throw 'Use native Windows PowerShell/cargo/Node.' }
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = "$env:TEMP\aeye-target-desktop" }
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }
if ($env:CARGO_TARGET_DIR.StartsWith('\\')) { throw 'CARGO_TARGET_DIR must be a local Windows path, not UNC.' }
$triple = (rustc --print host-tuple).Trim()
if ($LASTEXITCODE -ne 0 -or $triple -ne 'x86_64-pc-windows-msvc') { throw 'This milestone targets x86_64-pc-windows-msvc.' }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).ProviderPath
Push-Location $PSScriptRoot
try {
    & cargo build --locked --release --manifest-path "$repo\spikes\part-09-mcp-stdio\Cargo.toml"
    if ($LASTEXITCODE -ne 0) { throw 'MCP sidecar build failed.' }
    New-Item -ItemType Directory -Force -Path 'src-tauri\binaries' | Out-Null
    Copy-Item "$env:CARGO_TARGET_DIR\release\spike-09-mcp-stdio.exe" "src-tauri\binaries\aeye-mcp-$triple.exe" -Force
    if (-not (Test-Path 'node_modules\@tauri-apps\cli\tauri.js')) {
        & npm install
        if ($LASTEXITCODE -ne 0) { throw 'Local Tauri CLI install failed.' }
    }
    $cliArgs = @('node_modules\@tauri-apps\cli\tauri.js', 'build')
    if ($NoBundle) { $cliArgs += '--no-bundle' }
    & node @cliArgs
    if ($LASTEXITCODE -ne 0) { throw 'Tauri build failed.' }
    Write-Host "Desktop: $env:CARGO_TARGET_DIR\release\aeye-desktop.exe"
} finally { Pop-Location }
