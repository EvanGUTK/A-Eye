# Regenerates the synthetic speech fixtures with the offline Windows SAPI voice
# (Microsoft David Desktop). No microphone, no network, no recorded human speech.
# Output: fixtures\<name>.wav (16 kHz, 16-bit, mono PCM) and fixtures\<name>.txt (the spoken text).
#
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File generate-fixtures.ps1 [-OutDir <path>]
param(
    [string]$OutDir = (Join-Path $PSScriptRoot 'fixtures')
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech

# Words only, no digits: whisper writes numbers as digits, which would need normalisation.
$Fixtures = [ordered]@{
    'screenshot'    = 'Take a screenshot of the failing test output.'
    'second-screen' = 'Open the settings panel on the second monitor.'
    'linker-error'  = 'The build failed with a linker error in the main module.'
    'cargo-test'    = 'Run cargo test and open the stack trace in the terminal.'
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)

foreach ($name in $Fixtures.Keys) {
    $wav = Join-Path $OutDir "$name.wav"
    $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
    try {
        $synth.SelectVoice('Microsoft David Desktop')
        $synth.SetOutputToWaveFile($wav, $format)
        $synth.Speak($Fixtures[$name])
    } finally {
        $synth.Dispose() # flushes and closes the WAV
    }
    # UTF-8 without BOM, no trailing newline.
    [System.IO.File]::WriteAllText((Join-Path $OutDir "$name.txt"), $Fixtures[$name], (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "wrote $wav ($((Get-Item $wav).Length) bytes)"
}
