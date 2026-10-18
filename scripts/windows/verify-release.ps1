# PowerShell script to verify that the staged Windows release satisfies all release contracts
param(
    [string]$AppDir
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path "$PSScriptRoot\..\.."
if (-not $AppDir) {
    $AppDir = "$RepoRoot\dist\Echolet"
}

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

$SpikeExePath = "$AppDir\echolet-ui-spike.exe"
if (Test-Path $SpikeExePath) {
    Write-Error "[Error] $SpikeExePath found in production release package!"
    exit 1
}

# 2. Check embedded application icon in executable
Write-Host "--> Checking embedded application icon in executable..."
$RunningOnWindows = if ($null -ne (Get-Variable -Name "IsWindows" -ErrorAction SilentlyContinue)) {
    [bool]$IsWindows
} else {
    ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT)
}

if ($RunningOnWindows) {
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public class IconCheckHelper {
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern uint ExtractIconEx(string lpszFile, int nIconIndex, IntPtr[] phiconLarge, IntPtr[] phiconSmall, uint nIcons);
}
"@ -ErrorAction SilentlyContinue

    try {
        $IconCount = [IconCheckHelper]::ExtractIconEx($ExePath, -1, $null, $null, 0)
        Write-Host "    Detected embedded icon count in echolet.exe: $IconCount"
        if ($IconCount -eq 0) {
            Write-Error "[Error] $ExePath has no embedded application icon! Application icon resource is missing."
            exit 1
        }
    } catch {
        Write-Host "    [Notice] ExtractIconEx check skipped ($($_.Exception.Message))."
    }
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

# 4. Check diagnostic doctor scripts
Write-Host "--> Checking diagnostic doctor scripts..."
$DoctorFiles = @("doctor.ps1", "doctor.cmd")
foreach ($doc in $DoctorFiles) {
    $TargetDoc = "$AppDir\$doc"
    if (!(Test-Path $TargetDoc) -or (Get-Item $TargetDoc).Length -eq 0) {
        Write-Error "[Error] Missing or empty diagnostic tool: $TargetDoc"
        exit 1
    }
}

# 5. PE inspection helper for architecture and imports verification
function Read-PeInfo {
    param([string]$FilePath)

    $stream = [System.IO.File]::Open($FilePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    $reader = New-Object System.IO.BinaryReader($stream)
    try {
        $dosMagic = $reader.ReadUInt16()
        if ($dosMagic -ne 0x5A4D) { return $null }

        $stream.Seek(0x3C, [System.IO.SeekOrigin]::Begin) | Out-Null
        $e_lfanew = $reader.ReadUInt32()
        if ($e_lfanew + 24 -gt $stream.Length) { return $null }

        $stream.Seek($e_lfanew, [System.IO.SeekOrigin]::Begin) | Out-Null
        $peSig = $reader.ReadUInt32()
        if ($peSig -ne 0x00004550) { return $null }

        $machine = $reader.ReadUInt16()
        $numSections = $reader.ReadUInt16()
        $stream.Seek(12, [System.IO.SeekOrigin]::Current) | Out-Null
        $optHeaderSize = $reader.ReadUInt16()
        $characteristics = $reader.ReadUInt16()

        $optMagic = $reader.ReadUInt16()
        $is64Bit = ($optMagic -eq 0x020B)

        $importedDlls = @()
        if ($optHeaderSize -gt 0) {
            $importRvaOffset = if ($is64Bit) { 120 } else { 104 }
            if ($optHeaderSize -ge ($importRvaOffset + 8)) {
                $stream.Seek($e_lfanew + 24 + $importRvaOffset, [System.IO.SeekOrigin]::Begin) | Out-Null
                $importRva = $reader.ReadUInt32()
                $importSize = $reader.ReadUInt32()

                if ($importRva -gt 0 -and $importSize -gt 0) {
                    $sectionTableOffset = $e_lfanew + 24 + $optHeaderSize
                    $sections = @()
                    for ($i = 0; $i -lt $numSections; $i++) {
                        $stream.Seek($sectionTableOffset + ($i * 40), [System.IO.SeekOrigin]::Begin) | Out-Null
                        $secNameBytes = $reader.ReadBytes(8)
                        $secName = [System.Text.Encoding]::ASCII.GetString($secNameBytes).TrimEnd("`0")
                        $vSize = $reader.ReadUInt32()
                        $vAddr = $reader.ReadUInt32()
                        $rSize = $reader.ReadUInt32()
                        $rPtr = $reader.ReadUInt32()
                        $sections += [PSCustomObject]@{
                            Name = $secName; VAddr = $vAddr; VSize = $vSize; RPtr = $rPtr; RSize = $rSize
                        }
                    }

                    $rvaToOffset = {
                        param($targetRva)
                        foreach ($sec in $sections) {
                            $span = [Math]::Max($sec.VSize, $sec.RSize)
                            if ($targetRva -ge $sec.VAddr -and $targetRva -lt ($sec.VAddr + $span)) {
                                return $sec.RPtr + ($targetRva - $sec.VAddr)
                            }
                        }
                        return $null
                    }

                    $importFileOffset = & $rvaToOffset $importRva
                    if ($null -ne $importFileOffset -and $importFileOffset -lt $stream.Length) {
                        $curDescOffset = $importFileOffset
                        while ($curDescOffset + 20 -le $stream.Length) {
                            $stream.Seek($curDescOffset, [System.IO.SeekOrigin]::Begin) | Out-Null
                            $iltRva = $reader.ReadUInt32()
                            $timestamp = $reader.ReadUInt32()
                            $forwarder = $reader.ReadUInt32()
                            $nameRva = $reader.ReadUInt32()
                            $iatRva = $reader.ReadUInt32()

                            if ($iltRva -eq 0 -and $nameRva -eq 0) { break }

                            $nameOffset = & $rvaToOffset $nameRva
                            if ($null -ne $nameOffset -and $nameOffset -lt $stream.Length) {
                                $stream.Seek($nameOffset, [System.IO.SeekOrigin]::Begin) | Out-Null
                                $nameBytes = New-Object System.Collections.Generic.List[byte]
                                while ($stream.Position -lt $stream.Length) {
                                    $b = $reader.ReadByte()
                                    if ($b -eq 0) { break }
                                    $nameBytes.Add($b)
                                }
                                $dllName = [System.Text.Encoding]::ASCII.GetString($nameBytes.ToArray())
                                if ($dllName) { $importedDlls += $dllName }
                            }
                            $curDescOffset += 20
                        }
                    }
                }
            }
        }

        return @{
            Machine = $machine
            Is64Bit = $is64Bit
            ImportedDlls = $importedDlls
        }
    } finally {
        $reader.Close()
        $stream.Close()
    }
}

Write-Host "--> Verifying PE bitness and architecture of shipped binaries..."
$BinariesToVerify = @("echolet.exe", "sherpa-onnx-c-api.dll", "onnxruntime.dll")
foreach ($binName in $BinariesToVerify) {
    $binPath = Join-Path $AppDir $binName
    if (Test-Path $binPath) {
        $pe = Read-PeInfo -FilePath $binPath
        if ($null -eq $pe) {
            Write-Error "[Error] $binName failed PE header validation!"
            exit 1
        }
        if ($pe.Machine -ne 0x8664 -or -not $pe.Is64Bit) {
            Write-Error "[Error] $binName is not a 64-bit AMD64 PE binary! Machine: $($pe.Machine), Is64Bit: $($pe.Is64Bit)"
            exit 1
        }
        Write-Host "    $($binName): Confirmed 64-bit AMD64 PE binary."
    }
}

# Check PE dependencies and resource section if dumpbin is available
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

# 6. Check model registry and catalog metadata
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

# 7. Assert that model payload files/directories are absent
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

if ((Test-Path "$AppDir\models\test_wavs") -or (Test-Path "$AppDir\models\bilingual-zh-en\test_wavs")) {
    Write-Error "[Error] test_wavs directory found in production release!"
    exit 1
}

# 8. Ensure no development residue (*.pdb, *.lib, *.exp)
$Residue = Get-ChildItem -Path $AppDir -Recurse -Include *.pdb, *.lib, *.exp
if ($Residue.Count -gt 0) {
    Write-Error "[Error] Development residue (*.pdb, *.lib, *.exp) found in production folder: $($Residue.FullName)"
    exit 1
}

# 9. Check license files
Write-Host "--> Checking license notices..."
$Licenses = @(
    "sherpa-onnx-LICENSE",
    "onnxruntime-LICENSE",
    "model-LICENSE",
    "lucide-LICENSE"
)
foreach ($lic in $Licenses) {
    $TargetLic = Join-Path (Join-Path $AppDir "licenses") $lic
    if (!(Test-Path $TargetLic) -or (Get-Item $TargetLic).Length -eq 0) {
        Write-Error "[Error] Missing or empty license file: $TargetLic"
        exit 1
    }
}

# 10. Run Doctor on staged package (baseline verification)
Write-Host "--> Running Doctor verification on staged package..."
$DoctorScript = Join-Path $AppDir "doctor.ps1"
if (Test-Path $DoctorScript) {
    & $DoctorScript -AppDir $AppDir -SkipProbe
    if ($LASTEXITCODE -ne 0) {
        Write-Error "[Error] Doctor verification failed on staged package (Exit code: $LASTEXITCODE)!"
        exit 1
    }
}

# 11. Acceptance Test Suite: Deliberately injected faults in temporary copies
Write-Host "--> Running Doctor fault-injection diagnostic tests in temporary copies..."
$TestTempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("echolet-doc-test-" + [System.Guid]::NewGuid().ToString())
try {
    New-Item -ItemType Directory -Force -Path $TestTempRoot | Out-Null

    # Test A: Missing sherpa-onnx-c-api.dll
    $caseA = Join-Path $TestTempRoot "case_missing_sherpa"
    Copy-Item -Path $AppDir -Destination $caseA -Recurse -Force
    Remove-Item -Force (Join-Path $caseA "sherpa-onnx-c-api.dll") -ErrorAction SilentlyContinue
    & $DoctorScript -AppDir $caseA -SkipProbe
    if ($LASTEXITCODE -eq 0) {
        Write-Error "[Error] Doctor failed to flag missing sherpa-onnx-c-api.dll as error!"
        exit 1
    }
    Write-Host "    [Test A Passed] Missing sherpa-onnx-c-api.dll correctly detected (exit code $LASTEXITCODE)."

    # Test B: Missing onnxruntime.dll
    $caseB = Join-Path $TestTempRoot "case_missing_ort"
    Copy-Item -Path $AppDir -Destination $caseB -Recurse -Force
    Remove-Item -Force (Join-Path $caseB "onnxruntime.dll") -ErrorAction SilentlyContinue
    & $DoctorScript -AppDir $caseB -SkipProbe
    if ($LASTEXITCODE -eq 0) {
        Write-Error "[Error] Doctor failed to flag missing onnxruntime.dll as error!"
        exit 1
    }
    Write-Host "    [Test B Passed] Missing onnxruntime.dll correctly detected (exit code $LASTEXITCODE)."

    # Test C: Missing echolet.exe
    $caseC = Join-Path $TestTempRoot "case_missing_exe"
    Copy-Item -Path $AppDir -Destination $caseC -Recurse -Force
    Remove-Item -Force (Join-Path $caseC "echolet.exe") -ErrorAction SilentlyContinue
    & $DoctorScript -AppDir $caseC -SkipProbe
    if ($LASTEXITCODE -eq 0) {
        Write-Error "[Error] Doctor failed to flag missing echolet.exe as error!"
        exit 1
    }
    Write-Host "    [Test C Passed] Missing echolet.exe correctly detected (exit code $LASTEXITCODE)."

    # Test D: Missing models\registry.json
    $caseD = Join-Path $TestTempRoot "case_missing_reg"
    Copy-Item -Path $AppDir -Destination $caseD -Recurse -Force
    Remove-Item -Force (Join-Path (Join-Path $caseD "models") "registry.json") -ErrorAction SilentlyContinue
    & $DoctorScript -AppDir $caseD -SkipProbe
    if ($LASTEXITCODE -eq 0) {
        Write-Error "[Error] Doctor failed to flag missing models\registry.json as error!"
        exit 1
    }
    Write-Host "    [Test D Passed] Missing models\registry.json correctly detected (exit code $LASTEXITCODE)."

    # Test E: Corrupted / invalid PE file
    $caseE = Join-Path $TestTempRoot "case_corrupt_dll"
    Copy-Item -Path $AppDir -Destination $caseE -Recurse -Force
    [System.IO.File]::WriteAllBytes((Join-Path $caseE "sherpa-onnx-c-api.dll"), [byte[]]@(0x00, 0x01, 0x02, 0x03, 0x04))
    & $DoctorScript -AppDir $caseE -SkipProbe
    if ($LASTEXITCODE -eq 0) {
        Write-Error "[Error] Doctor failed to flag corrupt PE DLL as error!"
        exit 1
    }
    Write-Host "    [Test E Passed] Corrupt PE DLL correctly detected (exit code $LASTEXITCODE)."

    # Test F: Execution from directory with spaces and outside working directory
    $caseF = Join-Path $TestTempRoot "Echolet Path With Spaces"
    Copy-Item -Path $AppDir -Destination $caseF -Recurse -Force
    & (Join-Path $caseF "doctor.ps1") -AppDir $caseF -SkipProbe
    if ($LASTEXITCODE -ne 0) {
        Write-Error "[Error] Doctor failed when executed on path with spaces (exit code $LASTEXITCODE)!"
        exit 1
    }
    Write-Host "    [Test F Passed] Path with spaces executed cleanly."
} finally {
    Remove-Item -Recurse -Force $TestTempRoot -ErrorAction SilentlyContinue
}

# 12. Print final package size
Write-Host "--> Staged package size:"
$PackageSize = (Get-ChildItem -Path $AppDir -Recurse | Measure-Object -Property Length -Sum).Sum
Write-Host "    Total package size: $([math]::Round($PackageSize / 1MB, 2)) MB"

Write-Host "=== All Echolet Windows release verification checks PASSED! ==="
