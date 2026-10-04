# PowerShell script to stage the Echolet Windows release folder at dist\Echolet
param(
    [string]$Architecture = "x64"
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path "$PSScriptRoot\..\.."
$DistDir = "$RepoRoot\dist"
$AppDir = "$DistDir\Echolet"
$LocalRuntime = "$RepoRoot\.local-runtime"

Write-Host "=== Building & Staging Echolet Windows Package ($Architecture) ==="
Write-Host "Repo root:  $RepoRoot"
Write-Host "App target: $AppDir"

# 1. Ensure local staging assets exist (runtime libraries only)
if (!(Test-Path "$LocalRuntime\runtime\lib\sherpa-onnx-c-api.lib") -or !(Test-Path "$LocalRuntime\runtime\bin\sherpa-onnx-c-api.dll")) {
    Write-Host "--> Local runtime assets not found. Running prepare-assets.ps1 -RuntimeOnly first..."
    & "$PSScriptRoot\prepare-assets.ps1" -Architecture $Architecture -RuntimeOnly
}

# 2. Build release binary
Write-Host "--> Compiling release binary with cargo build --release..."
Push-Location $RepoRoot
try {
    cargo build --release
}
finally {
    Pop-Location
}

# 3. Clean and recreate bundle directory layout
if (Test-Path $AppDir) {
    Remove-Item -Recurse -Force $AppDir
}

New-Item -ItemType Directory -Force -Path $AppDir | Out-Null
New-Item -ItemType Directory -Force -Path "$AppDir\models" | Out-Null
New-Item -ItemType Directory -Force -Path "$AppDir\licenses" | Out-Null

# 4. Copy executable
Write-Host "--> Copying executable..."
Copy-Item -Path "$RepoRoot\target\release\echolet.exe" -Destination "$AppDir\echolet.exe" -Force

# 5. Copy all runtime dynamic libraries (*.dll)
Write-Host "--> Copying native runtime DLLs..."
$DllSource = if (Test-Path "$LocalRuntime\runtime\bin") { "$LocalRuntime\runtime\bin" } else { "$LocalRuntime\runtime\lib" }
Copy-Item -Path "$DllSource\*.dll" -Destination $AppDir -Force

# 6. Copy model catalog metadata (registry.json only, no weights)
Write-Host "--> Copying model registry..."
Copy-Item -Path "$RepoRoot\models\registry.json" -Destination "$AppDir\models\registry.json" -Force

# 7. Copy licenses
Write-Host "--> Copying licenses..."
Copy-Item -Path "$RepoRoot\licenses\*" -Destination "$AppDir\licenses\" -Force

# 8. Sanity check bundle completeness
Write-Host "--> Validating production package structure..."
$RequiredFiles = @(
    "$AppDir\echolet.exe",
    "$AppDir\models\registry.json",
    "$AppDir\sherpa-onnx-c-api.dll",
    "$AppDir\onnxruntime.dll",
    "$AppDir\licenses\sherpa-onnx-LICENSE",
    "$AppDir\licenses\onnxruntime-LICENSE",
    "$AppDir\licenses\model-LICENSE",
    "$AppDir\licenses\lucide-LICENSE"
)

foreach ($f in $RequiredFiles) {
    if (!(Test-Path $f)) {
        Write-Error "[Error] Missing expected bundle file: $f"
        exit 1
    }
}

# Ensure model payload files and directories are absent
if (Test-Path "$AppDir\model.json") {
    Write-Error "[Error] root model.json found in production release!"
    exit 1
}

if (Test-Path "$AppDir\models\bilingual-zh-en") {
    Write-Error "[Error] models\bilingual-zh-en directory found in production release!"
    exit 1
}

$OnnxFiles = Get-ChildItem -Path $AppDir -Recurse -Filter "*.onnx"
if ($OnnxFiles.Count -gt 0) {
    Write-Error "[Error] ONNX model files found in production release!"
    exit 1
}

$TokensFiles = Get-ChildItem -Path $AppDir -Recurse -Filter "tokens.txt"
if ($TokensFiles.Count -gt 0) {
    Write-Error "[Error] tokens.txt found in production release!"
    exit 1
}

if ((Test-Path "$AppDir\models\test_wavs") -or (Test-Path "$AppDir\models\bilingual-zh-en\test_wavs")) {
    Write-Error "[Error] test_wavs directory found in production release!"
    exit 1
}

Write-Host "=== Echolet Windows release staged successfully at: $AppDir ==="
Get-ChildItem -Path $AppDir
Get-ChildItem -Path "$AppDir\models"
Get-ChildItem -Path "$AppDir\licenses"
