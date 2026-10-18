<#
.SYNOPSIS
    Echolet Windows Diagnostics Doctor.
.DESCRIPTION
    A lightweight, standalone, read-only diagnostic utility for Echolet on Windows.
    Inspects bundle completeness, PE binary headers and architecture, Visual C++
    runtime availability, and recent Windows Error Reporting / Application Error events.
    Does NOT modify system settings, environment variables, PATH, or registry.
    Does NOT require administrator privileges or network connectivity.
    Compatible with Windows PowerShell 5.1 and PowerShell Core (pwsh).
.PARAMETER AppDir
    Optional root directory of the Echolet bundle to inspect. Defaults to the directory
    containing this script ($PSScriptRoot).
.PARAMETER SkipProbe
    Optional switch to skip the live subprocess launch probe.
#>
param(
    [string]$AppDir = $PSScriptRoot,
    [switch]$SkipProbe
)

$ErrorActionPreference = "Continue"

# Resolve absolute path of the bundle directory
if (-not $AppDir) {
    $AppDir = $PSScriptRoot
}
if (-not $AppDir) {
    $AppDir = (Get-Location).Path
}

try {
    $AppDir = (Resolve-Path -LiteralPath $AppDir).Path
} catch {
    # In case path does not exist yet
}

$OfficialVcRedistUrl = "https://aka.ms/vs/17/release/vc_redist.x64.exe"
$OfficialReleaseUrl = "https://github.com/SentimentalK/echolet/releases"

$PassCount = 0
$WarnCount = 0
$FailCount = 0

function Write-Section {
    param([string]$Title)
    Write-Host "`n[$Title]" -ForegroundColor Cyan
}

function Report-Pass {
    param([string]$Message)
    $script:PassCount++
    Write-Host "  [PASS] $Message" -ForegroundColor Green
}

function Report-Warn {
    param([string]$Message, [string]$Remediation = "")
    $script:WarnCount++
    Write-Host "  [WARN] $Message" -ForegroundColor Yellow
    if ($Remediation) {
        Write-Host "         Remediation: $Remediation" -ForegroundColor DarkYellow
    }
}

function Report-Fail {
    param([string]$Message, [string]$Remediation = "")
    $script:FailCount++
    Write-Host "  [FAIL] $Message" -ForegroundColor Red
    if ($Remediation) {
        Write-Host "         Remediation: $Remediation" -ForegroundColor Magenta
    }
}

function Get-PeBinaryInfo {
    param([string]$FilePath)

    if (-not (Test-Path -LiteralPath $FilePath)) {
        return @{ Status = "Missing"; Error = "File does not exist" }
    }

    $fileItem = Get-Item -LiteralPath $FilePath -ErrorAction SilentlyContinue
    if (-not $fileItem -or $fileItem.Length -lt 256) {
        $len = if ($fileItem) { $fileItem.Length } else { 0 }
        return @{ Status = "Corrupt"; Error = "File size ($len bytes) is too small to be a valid PE binary" }
    }

    $stream = $null
    $reader = $null
    try {
        $stream = [System.IO.File]::Open($FilePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
        $reader = New-Object System.IO.BinaryReader($stream)

        # 1. DOS Header ('MZ')
        $dosMagic = $reader.ReadUInt16()
        if ($dosMagic -ne 0x5A4D) {
            return @{ Status = "Invalid"; Error = "Invalid DOS header magic (not a Windows PE executable)" }
        }

        # Offset to PE header at 0x3C
        $stream.Seek(0x3C, [System.IO.SeekOrigin]::Begin) | Out-Null
        $e_lfanew = $reader.ReadUInt32()

        if ($e_lfanew + 24 -gt $stream.Length) {
            return @{ Status = "Corrupt"; Error = "PE header offset ($e_lfanew) points outside file bounds" }
        }

        # 2. PE Signature ('PE\0\0')
        $stream.Seek($e_lfanew, [System.IO.SeekOrigin]::Begin) | Out-Null
        $peSig = $reader.ReadUInt32()
        if ($peSig -ne 0x00004550) {
            return @{ Status = "Invalid"; Error = "Invalid PE signature" }
        }

        # 3. COFF File Header
        $machine = $reader.ReadUInt16()
        $numSections = $reader.ReadUInt16()
        $timeDateStamp = $reader.ReadUInt32()
        $ptrSymbolTable = $reader.ReadUInt32()
        $numSymbols = $reader.ReadUInt32()
        $optHeaderSize = $reader.ReadUInt16()
        $characteristics = $reader.ReadUInt16()

        # 4. Optional Header Magic
        $optMagic = $reader.ReadUInt16()
        $is64Bit = ($optMagic -eq 0x020B) # PE32+
        $is32Bit = ($optMagic -eq 0x010B) # PE32

        $machineName = switch ($machine) {
            0x8664 { "AMD64 (x86_64)" }
            0x014C { "i386 (x86 32-bit)" }
            0xAA64 { "ARM64" }
            0x01C0 { "ARM Thumb" }
            default { "Unknown (0x$($machine.ToString('X4')))" }
        }

        # 5. Extract Imported DLLs from Import Directory Table
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
            Status = "OK"
            Machine = $machine
            MachineName = $machineName
            Is64Bit = $is64Bit
            Is32Bit = $is32Bit
            ImportedDlls = $importedDlls
            Size = $fileItem.Length
        }
    } catch {
        return @{ Status = "Error"; Error = $_.Exception.Message }
    } finally {
        if ($null -ne $reader) { $reader.Close() }
        if ($null -ne $stream) { $stream.Close() }
    }
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host " Echolet Windows Diagnostics Doctor" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Bundle Directory: $AppDir"
Write-Host "Timestamp:        $((Get-Date).ToString('yyyy-MM-dd HH:mm:ss zzz'))"
Write-Host "PowerShell:       $($PSVersionTable.PSVersion)"
Write-Host "OS Platform:      $([System.Environment]::OSVersion.VersionString)"

# -------------------------------------------------------------
# Check 1: Operating System & Architecture
# -------------------------------------------------------------
Write-Section "1. Operating System Architecture"

$RunningOnWindows = $false
try {
    if ($null -ne (Get-Variable -Name "IsWindows" -ErrorAction SilentlyContinue)) {
        $RunningOnWindows = [bool]$IsWindows
    } else {
        $RunningOnWindows = ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT)
    }
} catch {
    $RunningOnWindows = $false
}

if (-not $RunningOnWindows) {
    Report-Warn "Running on a non-Windows platform ($([System.Environment]::OSVersion.Platform)). Windows-specific system registry, System32, and event log checks will be skipped or simulated."
} else {
    $is64BitOs = $false
    try {
        $is64BitOs = [Environment]::Is64BitOperatingSystem
    } catch {
        if ($env:PROCESSOR_ARCHITECTURE -eq "AMD64" -or $env:PROCESSOR_ARCHITEW6432 -eq "AMD64") {
            $is64BitOs = $true
        }
    }

    if ($is64BitOs) {
        Report-Pass "64-bit Windows operating system confirmed ($env:PROCESSOR_ARCHITECTURE)."
    } else {
        Report-Fail "32-bit Windows operating system detected. Echolet requires a 64-bit (x64) Windows environment." "Please run Echolet on a 64-bit Windows installation."
    }

    try {
        if ([Environment]::Is64BitProcess) {
            Report-Pass "PowerShell execution environment is 64-bit process."
        } else {
            Report-Warn "PowerShell is running as a 32-bit (x86) process on a 64-bit OS. System32 redirection (SysWOW64) may affect file lookups."
        }
    } catch {}
}

# -------------------------------------------------------------
# Check 2: Bundle Structure & File Completeness
# -------------------------------------------------------------
Write-Section "2. Bundle Structure & File Completeness"

if (-not (Test-Path -LiteralPath $AppDir)) {
    Report-Fail "Target bundle directory does not exist: $AppDir" "Verify the path or re-extract the official release ZIP from $OfficialReleaseUrl."
} else {
    $criticalFiles = @(
        @{ Name = "echolet.exe"; Path = Join-Path $AppDir "echolet.exe" },
        @{ Name = "sherpa-onnx-c-api.dll"; Path = Join-Path $AppDir "sherpa-onnx-c-api.dll" },
        @{ Name = "onnxruntime.dll"; Path = Join-Path $AppDir "onnxruntime.dll" },
        @{ Name = "models\registry.json"; Path = Join-Path $AppDir "models\registry.json" }
    )

    $missingCritical = @()
    foreach ($item in $criticalFiles) {
        $path = $item.Path
        if (-not (Test-Path -LiteralPath $path)) {
            $missingCritical += $item.Name
            Report-Fail "Missing essential bundle file: $($item.Name)" "Re-extract the entire archive into a standard folder. Files should not be launched directly inside the compressed ZIP preview."
        } else {
            $itemSize = (Get-Item -LiteralPath $path).Length
            if ($itemSize -eq 0) {
                $missingCritical += $item.Name
                Report-Fail "Empty (0 bytes) bundle file: $($item.Name)" "The file appears damaged or incomplete. Re-download from $OfficialReleaseUrl."
            } else {
                Report-Pass "Found $($item.Name) ($([math]::Round($itemSize / 1MB, 2)) MB)."
            }
        }
    }

    # Model catalog JSON sanity check
    $registryPath = Join-Path $AppDir "models\registry.json"
    if (Test-Path -LiteralPath $registryPath) {
        try {
            $registryRaw = Get-Content -LiteralPath $registryPath -Raw -ErrorAction Stop
            $registryJson = $registryRaw | ConvertFrom-Json -ErrorAction Stop
            if ($registryJson.models -and $registryJson.models.Count -gt 0) {
                Report-Pass "Model registry parsed successfully ($($registryJson.models.Count) models defined)."
            } else {
                Report-Warn "Model registry parsed, but contains no models list."
            }
        } catch {
            Report-Fail "models\registry.json is corrupt or not valid JSON: $($_.Exception.Message)" "Re-download the official release archive from $OfficialReleaseUrl."
        }
    }

    # Check for expected license notices (non-blocking)
    $expectedLicenses = @("sherpa-onnx-LICENSE", "onnxruntime-LICENSE", "model-LICENSE", "lucide-LICENSE")
    $missingLicenses = @()
    foreach ($lic in $expectedLicenses) {
        $licPath = Join-Path $AppDir "licenses\$lic"
        if (-not (Test-Path -LiteralPath $licPath)) {
            $missingLicenses += $lic
        }
    }
    if ($missingLicenses.Count -gt 0) {
        Report-Warn "Missing license notice file(s): $($missingLicenses -join ', ')."
    } else {
        Report-Pass "All expected third-party license notices present in licenses/ directory."
    }

    # Verify absence of staged model payload (model on demand design)
    $stagedModels = Join-Path $AppDir "models\bilingual-zh-en"
    if (Test-Path -LiteralPath $stagedModels) {
        Report-Warn "Pre-staged model directory found ($stagedModels). Echolet uses a model-on-demand design."
    }
}

# -------------------------------------------------------------
# Check 3: PE Binary Headers & Architecture (Safe Offline)
# -------------------------------------------------------------
Write-Section "3. Portable Executable (PE) Headers & Architecture"

$binariesToCheck = @(
    @{ Name = "echolet.exe"; Path = Join-Path $AppDir "echolet.exe"; ExpectImports = @("sherpa-onnx-c-api.dll", "onnxruntime.dll") },
    @{ Name = "sherpa-onnx-c-api.dll"; Path = Join-Path $AppDir "sherpa-onnx-c-api.dll"; ExpectImports = @("onnxruntime.dll") },
    @{ Name = "onnxruntime.dll"; Path = Join-Path $AppDir "onnxruntime.dll"; ExpectImports = @() }
)

foreach ($bin in $binariesToCheck) {
    if (-not (Test-Path -LiteralPath $bin.Path)) {
        continue
    }

    $peInfo = Get-PeBinaryInfo -FilePath $bin.Path
    if ($peInfo.Status -ne "OK") {
        Report-Fail "$($bin.Name): PE parsing failed - $($peInfo.Error)" "The file may be corrupt or not a Windows executable. Re-extract the release archive."
        continue
    }

    # Architecture verification: Must be AMD64 (0x8664) and PE32+ (64-bit)
    if ($peInfo.Machine -eq 0x8664 -and $peInfo.Is64Bit) {
        Report-Pass "$($bin.Name): Valid 64-bit AMD64 PE binary."
    } else {
        Report-Fail "$($bin.Name): Architecture mismatch. Detected $($peInfo.MachineName) (64-bit: $($peInfo.Is64Bit)). Echolet requires x64 (AMD64)." "Ensure you downloaded the 'echolet-windows-x64.zip' release package."
    }

    # Check expected dynamic link imports
    if ($bin.ExpectImports.Count -gt 0) {
        $missingImports = @()
        foreach ($imp in $bin.ExpectImports) {
            $found = $false
            foreach ($actual in $peInfo.ImportedDlls) {
                if ($actual.ToLowerInvariant() -eq $imp.ToLowerInvariant()) {
                    $found = $true
                    break
                }
            }
            if (-not $found) {
                $missingImports += $imp
            }
        }
        if ($missingImports.Count -gt 0) {
            Report-Warn "$($bin.Name): Expected import(s) not listed in PE import table: $($missingImports -join ', ')."
        }
    }
}

# -------------------------------------------------------------
# Check 4: Visual C++ Runtime Availability
# -------------------------------------------------------------
Write-Section "4. Microsoft Visual C++ 2015-2022 (x64) Runtime Availability"

# Transitive runtime requirement:
# sherpa-onnx-c-api.dll and onnxruntime.dll dynamically link against:
#   MSVCP140.dll, MSVCP140_1.dll, VCRUNTIME140.dll, VCRUNTIME140_1.dll
# and the Universal CRT (api-ms-win-crt-* / ucrtbase.dll).
$requiredVcDlls = @("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll", "msvcp140_1.dll")

# 4a. Check App-Local Runtime Deployment
$appLocalFound = @()
$appLocalMissing = @()
foreach ($dll in $requiredVcDlls) {
    $targetPath = Join-Path $AppDir $dll
    if (Test-Path -LiteralPath $targetPath) {
        $appLocalFound += $dll
    } else {
        $appLocalMissing += $dll
    }
}

$hasAppLocalVc = ($appLocalMissing.Count -eq 0)
if ($hasAppLocalVc) {
    Report-Pass "Visual C++ runtime libraries present app-locally in bundle directory ($($appLocalFound -join ', '))."
} else {
    Write-Host "  [INFO] App-local VC++ runtime libraries not bundled (standard for thin portable release)." -ForegroundColor Gray
}

# 4b. Check System-Installed Visual C++ Runtime
$system32Found = @()
$system32Missing = @()

if ($RunningOnWindows) {
    $system32Dir = if ($env:SystemRoot) { Join-Path $env:SystemRoot "System32" } else { "C:\Windows\System32" }
    foreach ($dll in $requiredVcDlls) {
        $targetPath = Join-Path $system32Dir $dll
        if (Test-Path -LiteralPath $targetPath) {
            $verInfo = (Get-Item -LiteralPath $targetPath -ErrorAction SilentlyContinue).VersionInfo
            $fileVer = if ($verInfo) { $verInfo.FileVersion } else { "unknown" }
            $system32Found += "$dll ($fileVer)"
        } else {
            $system32Missing += $dll
        }
    }
}

# 4c. Check Registry for Visual C++ 2015-2022 (x64) installation
$vcRegFound = $false
$vcRegDetails = ""

if ($RunningOnWindows) {
    $regPathsToCheck = @(
        "HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\X64",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\X64"
    )

    foreach ($rp in $regPathsToCheck) {
        if (Test-Path $rp -ErrorAction SilentlyContinue) {
            try {
                $item = Get-ItemProperty -Path $rp -ErrorAction SilentlyContinue
                if ($item -and ($item.Installed -eq 1 -or $item.Version)) {
                    $vcRegFound = $true
                    $vcRegDetails = "Registered at $rp (Version: $($item.Version), BFlat: $($item.BFlat))"
                    break
                }
            } catch {}
        }
    }

    if (-not $vcRegFound) {
        # Fallback check in Uninstall entries
        $uninstallKeys = @(
            "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*",
            "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*"
        )
        foreach ($uk in $uninstallKeys) {
            try {
                $entries = Get-ItemProperty -Path $uk -ErrorAction SilentlyContinue
                foreach ($entry in $entries) {
                    if ($entry.DisplayName -match "Microsoft Visual C\+\+ 201[5-9].*Redistributable.*x64" -or
                        $entry.DisplayName -match "Microsoft Visual C\+\+ 202[0-9].*Redistributable.*x64") {
                        $vcRegFound = $true
                        $vcRegDetails = "$($entry.DisplayName) (Version: $($entry.DisplayVersion))"
                        break
                    }
                }
            } catch {}
            if ($vcRegFound) { break }
        }
    }
}

# Evaluation of VC++ Runtime availability:
if ($hasAppLocalVc) {
    Report-Pass "Microsoft Visual C++ runtime satisfied via app-local libraries."
} elseif ($RunningOnWindows -and $system32Missing.Count -eq 0 -and $vcRegFound) {
    Report-Pass "Microsoft Visual C++ 2015-2022 (x64) installed and registered in system."
    Write-Host "         Details: $vcRegDetails" -ForegroundColor Gray
} elseif ($RunningOnWindows -and $system32Missing.Count -eq 0) {
    Report-Warn "All required Visual C++ runtime DLLs found in System32 ($($system32Found -join ', ')), but standard registry key was not detected."
} elseif ($RunningOnWindows) {
    Report-Fail "Missing required Visual C++ 2015-2022 (x64) runtime component(s): $($system32Missing -join ', ')." "Download and install official Microsoft Visual C++ 2015-2022 Redistributable (x64) from:`n         $OfficialVcRedistUrl"
} else {
    Write-Host "  [INFO] Visual C++ system checks skipped on non-Windows environment." -ForegroundColor Gray
}

# -------------------------------------------------------------
# Check 5: Windows Event Log Inspection (echolet.exe crash history)
# -------------------------------------------------------------
Write-Section "5. Diagnostic Event Log Analysis (Recent Application Errors)"

$logged0xc0000142 = $false
$loggedCrashEvents = @()

if ($RunningOnWindows) {
    try {
        $startDate = (Get-Date).AddDays(-7)
        $events = Get-WinEvent -FilterHashtable @{
            LogName = "Application"
            ProviderName = @("Application Error", "Windows Error Reporting")
            StartTime = $startDate
        } -ErrorAction Stop

        foreach ($evt in $events) {
            $msg = $evt.Message
            if ($msg -match "echolet\.exe") {
                $loggedCrashEvents += $evt
                if ($msg -match "c0000142" -or $msg -match "0xc0000142") {
                    $logged0xc0000142 = $true
                }
            }
        }
    } catch {
        # Keep event-log access failures non-fatal (e.g., service disabled or restricted permissions)
        Write-Host "  [INFO] Windows Event Log query unavailable or yielded no results: $($_.Exception.Message)" -ForegroundColor Gray
    }

    if ($loggedCrashEvents.Count -gt 0) {
        Report-Warn "Found $($loggedCrashEvents.Count) recent crash/error event(s) for echolet.exe in Windows Event Log."
        foreach ($evt in $loggedCrashEvents | Select-Object -First 3) {
            $timeStr = $evt.TimeCreated.ToString("yyyy-MM-dd HH:mm:ss")
            $firstLine = ($evt.Message -split "`r?`n")[0]
            Write-Host "         [$timeStr] Event ID $($evt.Id): $firstLine" -ForegroundColor DarkYellow
        }

        if ($logged0xc0000142) {
            Write-Host "`n  ============================================================" -ForegroundColor Red
            Write-Host "  DIAGNOSTIC NOTICE: STATUS_DLL_INIT_FAILED (0xC0000142) DETECTED" -ForegroundColor Red
            Write-Host "  ============================================================" -ForegroundColor Red
            Write-Host "  Windows recorded exit code 0xC0000142 for echolet.exe." -ForegroundColor Yellow
            Write-Host "  This code indicates that the Windows loader found the executable," -ForegroundColor Yellow
            Write-Host "  but one of its dependent DLLs failed during DllMain initialization" -ForegroundColor Yellow
            Write-Host "  before Rust application main() could execute." -ForegroundColor Yellow
            Write-Host "`n  [UNCERTAINTY NOTICE - Root cause is unconfirmed from logs alone]" -ForegroundColor Cyan
            Write-Host "  Known potential contributing causes and diagnostic hypotheses:" -ForegroundColor White
            Write-Host "   1. Missing or incompatible Visual C++ 2015-2022 (x64) Redistributable," -ForegroundColor White
            Write-Host "      especially vcruntime140_1.dll or msvcp140_1.dll." -ForegroundColor White
            Write-Host "      Remediation: Install $OfficialVcRedistUrl" -ForegroundColor Green
            Write-Host "   2. Corrupted, truncated, or blocked DLL files from incomplete ZIP extraction." -ForegroundColor White
            Write-Host "      Remediation: Delete folder, re-download ZIP, right-click -> Extract All." -ForegroundColor Green
            Write-Host "   3. Security software or AppInit_DLLs blocking injection or initialization." -ForegroundColor White
            Write-Host "   4. Conflicting third-party DLL on system PATH masking the bundled DLLs." -ForegroundColor White
            Write-Host "   5. CPU instruction set incompatibility (e.g. unsupported AVX instructions)." -ForegroundColor White
            Write-Host "  (Note: Launching with '--foreground' does NOT bypass pre-main loader failures.)" -ForegroundColor Gray
            Write-Host "  ============================================================`n" -ForegroundColor Red
        }
    } else {
        Report-Pass "No recent Application Error or WER crash events recorded for echolet.exe."
    }
} else {
    Write-Host "  [INFO] Event Log inspection skipped on non-Windows environment." -ForegroundColor Gray
}

# -------------------------------------------------------------
# Check 6: Subprocess Launch Probe (Isolated, Bounded Timeout)
# -------------------------------------------------------------
Write-Section "6. Subprocess Launch Probe"

$exePath = Join-Path $AppDir "echolet.exe"
if ($SkipProbe) {
    Write-Host "  [INFO] Subprocess probe skipped by user request (-SkipProbe)." -ForegroundColor Gray
} elseif (-not $RunningOnWindows) {
    Write-Host "  [INFO] Subprocess probe skipped on non-Windows environment." -ForegroundColor Gray
} elseif (-not (Test-Path -LiteralPath $exePath)) {
    Report-Fail "Cannot run subprocess probe: echolet.exe is missing from $AppDir."
} else {
    Write-Host "  --> Executing isolated 'echolet.exe status' subprocess probe (timeout: 5s)..." -ForegroundColor Gray

    $probeSuccess = $false
    $probeExitCode = $null
    $probeOutput = ""
    $probeError = ""

    try {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $exePath
        $psi.Arguments = "status"
        $psi.WorkingDirectory = $AppDir
        $psi.UseShellExecute = $false
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.CreateNoWindow = $true

        $proc = [System.Diagnostics.Process]::Start($psi)
        if ($proc.WaitForExit(5000)) {
            $probeExitCode = $proc.ExitCode
            $probeOutput = $proc.StandardOutput.ReadToEnd()
            $probeError = $proc.StandardError.ReadToEnd()
            $proc.Close()

            # Analyze Exit Code
            # STATUS_DLL_INIT_FAILED = 0xC0000142 = -1073741502
            # STATUS_DLL_NOT_FOUND   = 0xC0000135 = -1073741515
            # STATUS_ILLEGAL_INSTR   = 0xC000001D = -1073741795
            $hexExit = "0x" + ($probeExitCode -band 0xFFFFFFFF).ToString("X8")

            if ($probeExitCode -eq -1073741502 -or $hexExit -eq "0xC0000142") {
                Report-Fail "Subprocess probe FAILED with exit code $hexExit (STATUS_DLL_INIT_FAILED)." "One of the dependent DLLs failed during DllMain initialization.`n         Install official Microsoft Visual C++ 2015-2022 Redistributable: $OfficialVcRedistUrl`n         or verify archive extraction integrity."
            } elseif ($probeExitCode -eq -1073741515 -or $hexExit -eq "0xC0000135") {
                Report-Fail "Subprocess probe FAILED with exit code $hexExit (STATUS_DLL_NOT_FOUND)." "A required dependency DLL could not be located by the Windows loader.`n         Verify that sherpa-onnx-c-api.dll and onnxruntime.dll are present alongside echolet.exe."
            } elseif ($probeExitCode -eq 0) {
                Report-Pass "Subprocess probe SUCCEEDED (Exit Code 0: Echolet responded to status query)."
                $probeSuccess = $true
            } elseif ($probeExitCode -eq 1 -and ($probeError -match "Echolet daemon is not running" -or $probeOutput -match "Echolet daemon is not running")) {
                # Exit code 1 with clean named pipe error confirms process entered Rust main() successfully
                Report-Pass "Subprocess probe reached application runtime cleanly (Exit Code 1: Named pipe listener not resident, loader initialization succeeded)."
                $probeSuccess = $true
            } else {
                Report-Warn "Subprocess probe exited with non-standard code $hexExit.`n         Stdout: $probeOutput`n         Stderr: $probeError"
            }
        } else {
            try { $proc.Kill() } catch {}
            $proc.Close()
            Report-Warn "Subprocess probe timed out after 5 seconds and was terminated."
        }
    } catch {
        Report-Fail "Failed to spawn subprocess probe: $($_.Exception.Message)"
    }
}

# -------------------------------------------------------------
# Summary & Exit Status
# -------------------------------------------------------------
Write-Host "`n============================================================" -ForegroundColor Cyan
Write-Host " Diagnostic Summary: $PassCount PASSED, $WarnCount WARNINGS, $FailCount FAILED" -ForegroundColor $(if ($FailCount -gt 0) { "Red" } elseif ($WarnCount -gt 0) { "Yellow" } else { "Green" })
Write-Host "============================================================" -ForegroundColor Cyan

if ($FailCount -gt 0) {
    Write-Host "`n[RESULT: FAIL] Confirmed blocking issues detected that prevent Echolet from running." -ForegroundColor Red
    Write-Host "Please follow the remediation steps listed above.`n" -ForegroundColor Red
    exit 1
} elseif ($WarnCount -gt 0) {
    Write-Host "`n[RESULT: WARN] Potential issues or unconfirmed warnings detected." -ForegroundColor Yellow
    Write-Host "Echolet may still run, but review the warnings above if you experience issues.`n" -ForegroundColor Yellow
    exit 0
} else {
    Write-Host "`n[RESULT: PASS] All diagnostic checks passed. Bundle and environment are ready.`n" -ForegroundColor Green
    exit 0
}
