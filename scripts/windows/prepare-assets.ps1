# PowerShell script to prepare native Windows x64 runtime and base model assets
param(
    [string]$Architecture = "x64"
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path "$PSScriptRoot\..\.."
$StagingDir = "$RepoRoot\.local-runtime"
$RuntimeLibDir = "$StagingDir\runtime\lib"
$RuntimeBinDir = "$StagingDir\runtime\bin"
$ModelDir = "$StagingDir\models\bilingual-zh-en"

$SherpaVersion = "v1.13.6"
$SherpaAsset = "sherpa-onnx-v1.13.6-win-x64-shared-MD-Release-lib.tar.bz2"
$SherpaSha256 = "dca033829d3a7e74c127fc0d349a12257fb890fe5038a381ab1706e4b35cf0fa"
$SherpaUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/$SherpaVersion/$SherpaAsset"

# Read official model lock as single source of truth
$ModelLockPath = Join-Path $RepoRoot "models\base-model.lock.json"
$ModelLock = Get-Content $ModelLockPath -Raw | ConvertFrom-Json

$ModelUrl = $ModelLock.url
$ModelSha256 = $ModelLock.sha256
$ModelArchive = $ModelLock.archive

$TestWavUrl = "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20/resolve/main/test_wavs/0.wav"
$TestWavSha256 = "7d93384ca14702cc584a7a33fe2fed92e89e708549161cb12ea38c916882103b"

Write-Host "=== Downloading & Verifying Official Echolet Windows Assets ($Architecture) ==="
Write-Host "Repo root:   $RepoRoot"
Write-Host "Staging dir: $StagingDir"

New-Item -ItemType Directory -Force -Path $RuntimeLibDir | Out-Null
New-Item -ItemType Directory -Force -Path $RuntimeBinDir | Out-Null
New-Item -ItemType Directory -Force -Path "$ModelDir\test_wavs" | Out-Null
New-Item -ItemType Directory -Force -Path "$RepoRoot\dist" | Out-Null

$TempDir = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), [System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

try {
    # 1. Download and verify official sherpa-onnx Windows runtime
    $SherpaArchive = "$TempDir\$SherpaAsset"
    Write-Host "--> Downloading official sherpa-onnx runtime ($SherpaVersion for Windows x64)..."
    curl.exe -L --fail --retry 3 --retry-delay 2 -s -o $SherpaArchive $SherpaUrl

    Write-Host "--> Verifying SHA256 of sherpa runtime archive..."
    $CalcSha = (Get-FileHash -Algorithm SHA256 $SherpaArchive).Hash.ToLower()
    if ($CalcSha -ne $SherpaSha256.ToLower()) {
        Write-Error "[Error] SHA256 mismatch for $SherpaAsset! Expected: $SherpaSha256, Got: $CalcSha"
        exit 1
    }
    Write-Host "--> SHA256 verified: OK"

    Write-Host "--> Extracting native libraries into .local-runtime/runtime/..."
    tar.exe -xjf $SherpaArchive -C $TempDir
    $ExtractedLib = "$TempDir\sherpa-onnx-v1.13.6-win-x64-shared-MD-Release-lib\lib"

    Copy-Item -Path "$ExtractedLib\*.lib" -Destination $RuntimeLibDir -Force
    Copy-Item -Path "$ExtractedLib\*.dll" -Destination $RuntimeLibDir -Force
    Copy-Item -Path "$ExtractedLib\*.dll" -Destination $RuntimeBinDir -Force

    # 2. Acquire Echolet Base Model from lock file
    $CachedArchive = Join-Path "$RepoRoot\dist" $ModelArchive
    if (!(Test-Path $CachedArchive)) {
        Write-Host "--> Downloading Echolet Base Model ($ModelArchive)..."
        curl.exe -L --fail --retry 3 --retry-delay 2 -s -o $CachedArchive $ModelUrl
    }

    Write-Host "--> Verifying Echolet Base Model SHA256..."
    $ModelCalcSha = (Get-FileHash -Algorithm SHA256 $CachedArchive).Hash.ToLower()
    if ($ModelCalcSha -ne $ModelSha256.ToLower()) {
        Write-Error "[Error] SHA256 mismatch for Base Model! Expected: $ModelSha256, Got: $ModelCalcSha"
        exit 1
    }
    Write-Host "--> Base Model SHA256 verified: OK"

    Write-Host "--> Extracting Base Model into .local-runtime/models/bilingual-zh-en/..."
    if (Get-Command zstd -ErrorAction SilentlyContinue) {
        zstd -d -c $CachedArchive | tar.exe -xf - -C $ModelDir
    } else {
        tar.exe --zstd -xf $CachedArchive -C $ModelDir
    }

    # 3. Download test wav for stream testing
    $TestWavPath = "$ModelDir\test_wavs\0.wav"
    if (!(Test-Path $TestWavPath)) {
        Write-Host "--> Downloading test wav..."
        curl.exe -L --fail --retry 3 --retry-delay 2 -s -o $TestWavPath $TestWavUrl
    }
    $WavCalcSha = (Get-FileHash -Algorithm SHA256 $TestWavPath).Hash.ToLower()
    if ($WavCalcSha -ne $TestWavSha256.ToLower()) {
        Write-Error "[Error] SHA256 mismatch for test wav!"
        exit 1
    }

    # 4. Copy manifest and registry
    Copy-Item -Path "$RepoRoot\model.json" -Destination "$ModelDir\model.json" -Force
    Copy-Item -Path "$RepoRoot\model.json" -Destination "$StagingDir\model.json" -Force
    New-Item -ItemType Directory -Force -Path "$StagingDir\models" | Out-Null
    Copy-Item -Path "$RepoRoot\models\registry.json" -Destination "$StagingDir\models\registry.json" -Force

    Write-Host "=== Official Windows assets staged successfully into .local-runtime/ ==="
    Get-ChildItem -Path $RuntimeLibDir
    Get-ChildItem -Path $ModelDir
}
finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
