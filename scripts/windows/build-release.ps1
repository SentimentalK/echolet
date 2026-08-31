# PowerShell script to orchestrate the build, verification, and packaging of echolet-windows-${Architecture}.zip
param(
    [string]$Architecture = "x64"
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
& "$PSScriptRoot\verify-release.ps1"

# 3. Create zip archive
Write-Host "--> Creating release archive: $ArchivePath..."
if (Test-Path $ArchivePath) {
    Remove-Item -Force $ArchivePath
}

Compress-Archive -Path "$DistDir\Echolet\*" -DestinationPath $ArchivePath -Force

Write-Host "============================================================"
Write-Host " Windows Release Build Complete ($Architecture)!"
Write-Host " Output Archive: $ArchivePath"
$FileInfo = Get-Item $ArchivePath
Write-Host " Size: $([math]::Round($FileInfo.Length / 1MB, 2)) MB"
$Hash = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash.ToLower()
Write-Host " SHA256 Checksum: $Hash"
Write-Host "============================================================"
