# PowerShell script to orchestrate the build, verification, and packaging of echolet-windows-${Architecture}.zip
param(
    [string]$Architecture = "x64",
    [switch]$SkipSmokeTest
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path "$PSScriptRoot\..\.."
$DistDir = "$RepoRoot\dist"
$ArchiveName = "echolet-windows-$Architecture.zip"
$ArchivePath = "$DistDir\$ArchiveName"

Write-Host "============================================================"
Write-Host " Building Echolet Windows Portable Release ($Architecture)"
Write-Host "============================================================"

# 1. Stage release
& "$PSScriptRoot\stage-release.ps1" -Architecture $Architecture

# 2. Verify staged package
& "$PSScriptRoot\verify-release.ps1" -AppDir "$DistDir\Echolet"

# 3. Create zip archive
Write-Host "--> Creating release archive: $ArchivePath..."
if (Test-Path $ArchivePath) {
    Remove-Item -Force $ArchivePath
}

Compress-Archive -Path "$DistDir\Echolet\*" -DestinationPath $ArchivePath -Force

# 4. Verify freshly generated ZIP archive in an isolated location
Write-Host "--> Extracting freshly generated archive to isolated temporary location for verification..."
$TempExtract = Join-Path ([System.IO.Path]::GetTempPath()) ("echolet-zip-smoke-" + [System.Guid]::NewGuid().ToString())

try {
    New-Item -ItemType Directory -Force -Path $TempExtract | Out-Null
    Expand-Archive -LiteralPath $ArchivePath -DestinationPath $TempExtract -Force

    Write-Host "--> Verifying extracted archive contents..."
    $ExpectedFiles = @(
        "echolet.exe",
        "sherpa-onnx-c-api.dll",
        "onnxruntime.dll",
        "doctor.ps1",
        "doctor.cmd",
        (Join-Path "models" "registry.json"),
        (Join-Path "licenses" "sherpa-onnx-LICENSE"),
        (Join-Path "licenses" "onnxruntime-LICENSE"),
        (Join-Path "licenses" "model-LICENSE"),
        (Join-Path "licenses" "lucide-LICENSE")
    )
    foreach ($f in $ExpectedFiles) {
        $p = Join-Path $TempExtract $f
        if (!(Test-Path -LiteralPath $p)) {
            Write-Error "[Error] Freshly extracted ZIP missing required file: $f"
            exit 1
        }
    }

    # Run Doctor on extracted directory
    Write-Host "--> Executing Doctor on extracted ZIP package..."
    & (Join-Path $TempExtract "doctor.ps1") -AppDir $TempExtract -SkipProbe
    if ($LASTEXITCODE -ne 0) {
        Write-Error "[Error] Doctor verification failed on extracted ZIP package (exit code $LASTEXITCODE)!"
        exit 1
    }

    # 5. Isolated Process Launch Smoke Test
    $RunningOnWindows = if ($null -ne (Get-Variable -Name "IsWindows" -ErrorAction SilentlyContinue)) {
        [bool]$IsWindows
    } else {
        ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT)
    }

    if ($RunningOnWindows -and (-not $SkipSmokeTest)) {
        Write-Host "--> Running isolated startup smoke test on extracted echolet.exe..."
        Write-Host "    (Sanitizing PATH to exclude repo .local-runtime directories)"

        # Build sanitized PATH: strip repo directories and local runtime paths
        $cleanPathDirs = ($env:PATH -split ";") | Where-Object {
            $_ -and (-not ($_ -like "*$RepoRoot*")) -and (-not ($_ -like "*local-runtime*"))
        }
        $isolatedPath = $cleanPathDirs -join ";"

        $exeToTest = Join-Path $TempExtract "echolet.exe"

        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $exeToTest
        $psi.WorkingDirectory = $TempExtract
        $psi.UseShellExecute = $false
        $psi.EnvironmentVariables["PATH"] = $isolatedPath
        $psi.EnvironmentVariables["ECHOLET_DISABLE_BEEP"] = "1"
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.CreateNoWindow = $true

        $proc = [System.Diagnostics.Process]::Start($psi)
        $spawnedPid = $proc.Id
        Write-Host "    Spawned test process PID: $spawnedPid"

        $startedCleanly = $false
        $exitedEarly = $false
        $earlyExitCode = $null

        # Monitor startup progress up to 4 seconds
        for ($i = 0; $i -lt 8; $i++) {
            Start-Sleep -Milliseconds 500
            if ($proc.HasExited) {
                $exitedEarly = $true
                $earlyExitCode = $proc.ExitCode
                break
            }

            # Query named pipe to verify loader initialized and platform layer entered
            try {
                $statusPsi = New-Object System.Diagnostics.ProcessStartInfo
                $statusPsi.FileName = $exeToTest
                $statusPsi.Arguments = "status"
                $statusPsi.WorkingDirectory = $TempExtract
                $statusPsi.UseShellExecute = $false
                $statusPsi.EnvironmentVariables["PATH"] = $isolatedPath
                $statusPsi.RedirectStandardOutput = $true
                $statusPsi.RedirectStandardError = $true
                $statusPsi.CreateNoWindow = $true
                $statusProc = [System.Diagnostics.Process]::Start($statusPsi)
                if ($statusProc.WaitForExit(1000)) {
                    $statusOut = $statusProc.StandardOutput.ReadToEnd()
                    if ($statusProc.ExitCode -eq 0 -or $statusOut -match "running") {
                        $startedCleanly = $true
                        $statusProc.Close()
                        break
                    }
                    $statusProc.Close()
                }
            } catch {}
        }

        # Gracefully quit the resident application
        try {
            $quitPsi = New-Object System.Diagnostics.ProcessStartInfo
            $quitPsi.FileName = $exeToTest
            $quitPsi.Arguments = "quit"
            $quitPsi.WorkingDirectory = $TempExtract
            $quitPsi.UseShellExecute = $false
            $quitPsi.EnvironmentVariables["PATH"] = $isolatedPath
            $quitPsi.CreateNoWindow = $true
            $quitProc = [System.Diagnostics.Process]::Start($quitPsi)
            if ($quitProc.WaitForExit(2000)) {
                $quitProc.Close()
            }
        } catch {}

        # Ensure spawned test process is cleanly stopped
        if (-not $proc.HasExited) {
            try {
                Stop-Process -Id $spawnedPid -Force -ErrorAction SilentlyContinue
            } catch {}
            $proc.WaitForExit(1000)
        }
        $proc.Close()

        if ($exitedEarly) {
            $hexCode = "0x" + ($earlyExitCode -band 0xFFFFFFFF).ToString("X8")
            Write-Error "[Error] Extracted echolet.exe terminated prematurely during isolated smoke test with exit code $hexCode ($earlyExitCode)! Loader DLL initialization failed."
            exit 1
        }

        Write-Host "    Extracted echolet.exe startup succeeded and verified past loader initialization."
        Write-Host "    [Notice] CI hosted runner environment has Microsoft Visual C++ Redistributable pre-installed."
        Write-Host "    [Notice] Passing this isolated smoke test verifies archive bundling integrity and runner compatibility,"
        Write-Host "             but is NOT proof of clean consumer machine compatibility without the Visual C++ Redistributable."
    }
} finally {
    Remove-Item -Recurse -Force $TempExtract -ErrorAction SilentlyContinue
}

Write-Host "============================================================"
Write-Host " Windows Release Build Complete ($Architecture)!"
Write-Host " Output Archive: $ArchivePath"
$FileInfo = Get-Item $ArchivePath
Write-Host " Size: $([math]::Round($FileInfo.Length / 1MB, 2)) MB"
$Hash = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash.ToLower()
Write-Host " SHA256 Checksum: $Hash"
Write-Host "============================================================"
