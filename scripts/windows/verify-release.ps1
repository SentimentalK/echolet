# PowerShell script to verify that the staged Windows release satisfies all release contracts
$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path "$PSScriptRoot\..\.."
$AppDir = "$RepoRoot\dist\Echolet"

Write-Host "=== Verifying Echolet Windows Release Package ==="
Write-Host "Target directory: $AppDir"

if (!(Test-Path $AppDir)) {
    Write-Error "[Error] Staged Echolet release folder not found at: $AppDir"
    exit 1
}

# 1. Check binary executable
Write-Host "--> Checking binary executable..."
$ExePath = "$AppDir\echolet.exe"
if (!(Test-Path $ExePath) -or (Get-Item $ExePath).Length -eq 0) {
    Write-Error "[Error] $ExePath is missing or empty!"
    exit 1
}

# 2. Check essential DLL files
Write-Host "--> Checking essential native DLLs..."
$RequiredDlls = @(
    "sherpa-onnx-c-api.dll",
    "onnxruntime.dll"
)
foreach ($dll in $RequiredDlls) {
    $TargetDll = "$AppDir\$dll"
    if (!(Test-Path $TargetDll) -or (Get-Item $TargetDll).Length -eq 0) {
        Write-Error "[Error] Missing or empty runtime DLL: $TargetDll"
        exit 1
    }
}

# 3. Check PE dependencies if dumpbin is available
if (Get-Command dumpbin.exe -ErrorAction SilentlyContinue) {
    Write-Host "--> Checking PE dependencies with dumpbin..."
    $DumpOutput = dumpbin.exe /dependents $ExePath
    Write-Host $DumpOutput
}

# 4. Check model files, manifest, and registry
Write-Host "--> Checking model manifest, registry, and files..."
if (!(Test-Path "$AppDir\model.json")) {
    Write-Error "[Error] Missing model.json manifest!"
    exit 1
}
if (!(Test-Path "$AppDir\models\registry.json")) {
    Write-Error "[Error] Missing models\registry.json!"
    exit 1
}

$ModelFiles = @(
    "encoder-480ms.onnx",
    "decoder-480ms.onnx",
    "joiner-480ms.onnx",
    "tokens.txt"
)
foreach ($mf in $ModelFiles) {
    $TargetMf = "$AppDir\models\bilingual-zh-en\$mf"
    if (!(Test-Path $TargetMf) -or (Get-Item $TargetMf).Length -eq 0) {
        Write-Error "[Error] Missing or empty model file: $TargetMf"
        exit 1
    }
}

# 5. Ensure test_wavs is excluded
if (Test-Path "$AppDir\models\bilingual-zh-en\test_wavs") {
    Write-Error "[Error] test_wavs directory found in production release!"
    exit 1
}

# 6. Ensure no development residue (*.pdb, *.lib, *.exp)
$Residue = Get-ChildItem -Path $AppDir -Recurse -Include *.pdb, *.lib, *.exp
if ($Residue.Count -gt 0) {
    Write-Error "[Error] Development residue (*.pdb, *.lib, *.exp) found in production folder: $($Residue.FullName)"
    exit 1
}

# 7. Check license files
Write-Host "--> Checking license notices..."
$Licenses = @(
    "sherpa-onnx-LICENSE",
    "onnxruntime-LICENSE",
    "model-LICENSE"
)
foreach ($lic in $Licenses) {
    $TargetLic = "$AppDir\licenses\$lic"
    if (!(Test-Path $TargetLic) -or (Get-Item $TargetLic).Length -eq 0) {
        Write-Error "[Error] Missing or empty license file: $TargetLic"
        exit 1
    }
}

Write-Host "=== All Echolet Windows release verification checks PASSED! ==="
