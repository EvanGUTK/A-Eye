# Installs the pinned whisper.cpp and piper binaries plus models for spike 08.
# Network is used HERE only (downloads); the spike itself runs offline.
# Every download is verified against a pinned SHA-256 before use.
#
# Usage (Windows PowerShell):
#   powershell -NoProfile -ExecutionPolicy Bypass -File setup.ps1 [-ToolsDir <path>]
param(
    [string]$ToolsDir = (Join-Path $env:LOCALAPPDATA 'aeye-voice-tools')
)
$ErrorActionPreference = 'Stop'

$WhisperRelease = 'b5454' # CI build of whisper.cpp v1.9.5 (commit d1be6fde11ac6e0407606b4e42fe72d34add8037)
$VoicesRev = 'c10ece1aade47bb51c153c893d14e5bf8e5b7117' # rhasspy/piper-voices
$VoiceDir = 'en/en_US/lessac/medium'

$Downloads = @(
    @{ Name = 'whisper-bin-x64.zip'; Dest = 'whisper-bin-x64.zip'
       Url = "https://github.com/ggml-org/whisper.cpp/releases/download/$WhisperRelease/whisper-bin-x64.zip"
       Sha256 = '6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb' },
    @{ Name = 'piper_windows_amd64.zip'; Dest = 'piper_windows_amd64.zip'
       Url = 'https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip'
       Sha256 = 'f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea' },
    @{ Name = 'ggml-base.en.bin'; Dest = 'models\ggml-base.en.bin'
       Url = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin'
       Sha256 = 'a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002' },
    @{ Name = 'ggml-tiny.en.bin'; Dest = 'models\ggml-tiny.en.bin'
       Url = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.en.bin'
       Sha256 = '921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f' },
    @{ Name = 'en_US-lessac-medium.onnx'; Dest = 'models\en_US-lessac-medium.onnx'
       Url = "https://huggingface.co/rhasspy/piper-voices/resolve/$VoicesRev/$VoiceDir/en_US-lessac-medium.onnx"
       Sha256 = '5efe09e69902187827af646e1a6e9d269dee769f9877d17b16b1b46eeaaf019f' },
    @{ Name = 'en_US-lessac-medium.onnx.json'; Dest = 'models\en_US-lessac-medium.onnx.json'
       Url = "https://huggingface.co/rhasspy/piper-voices/resolve/$VoicesRev/$VoiceDir/en_US-lessac-medium.onnx.json"
       Sha256 = 'efe19c417bed055f2d69908248c6ba650fa135bc868b0e6abb3da181dab690a0' }
)

New-Item -ItemType Directory -Force -Path (Join-Path $ToolsDir 'models') | Out-Null

foreach ($d in $Downloads) {
    $path = Join-Path $ToolsDir $d.Dest
    if (-not (Test-Path $path)) {
        Write-Host "downloading $($d.Name)"
        # curl.exe ships with Windows 10+ and follows redirects without the progress-bar slowdown.
        & curl.exe -fsSL -o $path $d.Url
        if ($LASTEXITCODE -ne 0) {
            # A failed transfer can leave a partial file that the next run would otherwise skip.
            Remove-Item -Force -ErrorAction SilentlyContinue $path
            throw "download failed: $($d.Url)"
        }
    }
    $actual = (Get-FileHash -Algorithm SHA256 -Path $path).Hash.ToLowerInvariant()
    if ($actual -ne $d.Sha256) {
        Remove-Item $path
        throw "SHA-256 mismatch for $($d.Name): expected $($d.Sha256), got $actual (file removed)"
    }
    Write-Host "ok  $($d.Name)  sha256=$actual"
}

# Both zips contain a top-level directory: whisper\Release\ (whisper-cli.exe + dlls) and piper\ (piper.exe).
# The spike expects exactly these paths.
Expand-Archive -Force -Path (Join-Path $ToolsDir 'whisper-bin-x64.zip') -DestinationPath (Join-Path $ToolsDir 'whisper')
Expand-Archive -Force -Path (Join-Path $ToolsDir 'piper_windows_amd64.zip') -DestinationPath $ToolsDir

Write-Host "installed under $ToolsDir"
Write-Host "set AEYE_VOICE_TOOLS=$ToolsDir or pass --tools-dir"
