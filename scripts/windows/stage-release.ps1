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

# 1. Ensure local staging assets exist
if (!(Test-Path "$LocalRuntime\runtime\lib") -or !(Test-Path "$LocalRuntime\models\bilingual-zh-en\encoder-480ms.onnx")) {
    Write-Host "--> Local assets not found. Running prepare-assets.ps1 first..."
    & "$PSScriptRoot\prepare-assets.ps1" -Architecture $Architecture
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
New-Item -ItemType Directory -Force -Path "$AppDir\models\bilingual-zh-en" | Out-Null
New-Item -ItemType Directory -Force -Path "$AppDir\licenses" | Out-Null

# 4. Copy executable
Write-Host "--> Copying executable..."
Copy-Item -Path "$RepoRoot\target\release\echolet.exe" -Destination "$AppDir\echolet.exe" -Force

# 5. Copy all runtime dynamic libraries (*.dll)
Write-Host "--> Copying native runtime DLLs..."
$DllSource = if (Test-Path "$LocalRuntime\runtime\bin") { "$LocalRuntime\runtime\bin" } else { "$LocalRuntime\runtime\lib" }
Copy-Item -Path "$DllSource\*.dll" -Destination $AppDir -Force

# 6. Copy model files (excluding test_wavs)
Write-Host "--> Copying model files..."
Copy-Item -Path "$LocalRuntime\models\bilingual-zh-en\encoder-480ms.onnx" -Destination "$AppDir\models\bilingual-zh-en\" -Force
Copy-Item -Path "$LocalRuntime\models\bilingual-zh-en\decoder-480ms.onnx" -Destination "$AppDir\models\bilingual-zh-en\" -Force
Copy-Item -Path "$LocalRuntime\models\bilingual-zh-en\joiner-480ms.onnx" -Destination "$AppDir\models\bilingual-zh-en\" -Force
Copy-Item -Path "$LocalRuntime\models\bilingual-zh-en\tokens.txt" -Destination "$AppDir\models\bilingual-zh-en\" -Force

# 7. Copy model manifest and registry
Write-Host "--> Copying manifest and registry..."
Copy-Item -Path "$RepoRoot\model.json" -Destination "$AppDir\model.json" -Force
Copy-Item -Path "$RepoRoot\model.json" -Destination "$AppDir\models\bilingual-zh-en\model.json" -Force
Copy-Item -Path "$RepoRoot\models\registry.json" -Destination "$AppDir\models\registry.json" -Force

# 8. Copy licenses
Write-Host "--> Copying licenses..."
Copy-Item -Path "$RepoRoot\licenses\*" -Destination "$AppDir\licenses\" -Force

Write-Host "=== Echolet Windows release staged successfully at: $AppDir ==="
Get-ChildItem -Path $AppDir
Get-ChildItem -Path "$AppDir\models\bilingual-zh-en"
