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

# 2. Check embedded application icon in executable
Write-Host "--> Checking embedded application icon in executable..."
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public class IconCheckHelper {
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern uint ExtractIconEx(string lpszFile, int nIconIndex, IntPtr[] phiconLarge, IntPtr[] phiconSmall, uint nIcons);
}
"@ -ErrorAction SilentlyContinue

$IconCount = [IconCheckHelper]::ExtractIconEx($ExePath, -1, $null, $null, 0)
Write-Host "    Detected embedded icon count in echolet.exe: $IconCount"
if ($IconCount -eq 0) {
    Write-Error "[Error] $ExePath has no embedded application icon! Application icon resource is missing."
    exit 1
}

# 3. Check essential DLL files
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

# 4. Check PE dependencies and resource section if dumpbin is available
if (Get-Command dumpbin.exe -ErrorAction SilentlyContinue) {
    Write-Host "--> Checking PE dependencies with dumpbin..."
    $DumpOutput = dumpbin.exe /dependents $ExePath
    Write-Host $DumpOutput

    Write-Host "--> Checking PE headers for resource section (.rsrc)..."
    $HeaderOutput = dumpbin.exe /headers $ExePath
    if ($HeaderOutput -match "\.rsrc") {
        Write-Host "    Found .rsrc section in PE headers via dumpbin."
    } else {
        Write-Error "[Error] Missing .rsrc section in $ExePath according to dumpbin!"
        exit 1
    }
}

# 5. Check model registry and catalog metadata
Write-Host "--> Checking model registry and catalog metadata..."
if (!(Test-Path "$AppDir\models\registry.json")) {
    Write-Error "[Error] Missing models\registry.json!"
    exit 1
}

$RegistryContent = Get-Content "$AppDir\models\registry.json" -Raw
if ($RegistryContent -match '"bundled":\s*true') {
    Write-Error "[Error] models\registry.json still marks model as bundled: true!"
    exit 1
}
if ($RegistryContent -notmatch '"bundled":\s*false') {
    Write-Error "[Error] models\registry.json does not mark model as bundled: false!"
    exit 1
}

if (Test-Path "$AppDir\model.json") {
    Write-Error "[Error] root model.json found in production release!"
    exit 1
}

# 6. Assert that model payload files/directories are absent
Write-Host "--> Verifying absence of model weight payload..."
if (Test-Path "$AppDir\models\bilingual-zh-en") {
    Write-Error "[Error] models\bilingual-zh-en directory found in release bundle!"
    exit 1
}

$OnnxFiles = Get-ChildItem -Path $AppDir -Recurse -Filter "*.onnx"
if ($OnnxFiles.Count -gt 0) {
    Write-Error "[Error] ONNX model files found in release bundle!"
    exit 1
}

$TokensFiles = Get-ChildItem -Path $AppDir -Recurse -Filter "tokens.txt"
if ($TokensFiles.Count -gt 0) {
    Write-Error "[Error] tokens.txt found in release bundle!"
    exit 1
}

if (Test-Path "$AppDir\models\test_wavs" -or (Test-Path "$AppDir\models\bilingual-zh-en\test_wavs")) {
    Write-Error "[Error] test_wavs directory found in production release!"
    exit 1
}

# 7. Ensure no development residue (*.pdb, *.lib, *.exp)
$Residue = Get-ChildItem -Path $AppDir -Recurse -Include *.pdb, *.lib, *.exp
if ($Residue.Count -gt 0) {
    Write-Error "[Error] Development residue (*.pdb, *.lib, *.exp) found in production folder: $($Residue.FullName)"
    exit 1
}

# 8. Check license files
Write-Host "--> Checking license notices..."
$Licenses = @(
    "sherpa-onnx-LICENSE",
    "onnxruntime-LICENSE",
    "model-LICENSE",
    "lucide-LICENSE"
)
foreach ($lic in $Licenses) {
    $TargetLic = "$AppDir\licenses\$lic"
    if (!(Test-Path $TargetLic) -or (Get-Item $TargetLic).Length -eq 0) {
        Write-Error "[Error] Missing or empty license file: $TargetLic"
        exit 1
    }
}

# 9. Print final package size
Write-Host "--> Staged package size:"
$PackageSize = (Get-ChildItem -Path $AppDir -Recurse | Measure-Object -Property Length -Sum).Sum
Write-Host "    Total package size: $([math]::Round($PackageSize / 1MB, 2)) MB"

Write-Host "=== All Echolet Windows release verification checks PASSED! ==="
