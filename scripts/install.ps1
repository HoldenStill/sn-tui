$ErrorActionPreference = 'Stop'

$Repo = "HoldenStill/sn-tui"
$BinaryName = "sn-tui.exe"
$ArchiveName = "sn-tui-x86_64-pc-windows-msvc.zip"
$DownloadUrl = "https://github.com/$Repo/releases/latest/download/$ArchiveName"

$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\sn-tui"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$TempZip = Join-Path $env:TEMP $ArchiveName

Write-Host "Fetching latest sn-tui for Windows (x86_64) from GitHub..." -ForegroundColor Cyan
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -Uri $DownloadUrl -OutFile $TempZip -UseBasicParsing

Write-Host "Extracting archive..." -ForegroundColor Cyan
Expand-Archive -Path $TempZip -DestinationPath $InstallDir -Force
Remove-Item -Path $TempZip -Force -ErrorAction SilentlyContinue

$TargetExe = Join-Path $InstallDir $BinaryName
if (-not (Test-Path $TargetExe)) {
    # If zipped inside a subdirectory, move it up
    $FoundExe = Get-ChildItem -Path $InstallDir -Filter $BinaryName -Recurse | Select-Object -First 1
    if ($FoundExe) {
        Move-Item -Path $FoundExe.FullName -Destination $TargetExe -Force
    }
}

if (Test-Path $TargetExe) {
    Write-Host "Successfully installed sn-tui to: $TargetExe" -ForegroundColor Green

    # Check and add to User PATH if missing
    $UserPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
    $PathParts = $UserPath -split ';' | Where-Object { $_ -ne '' }
    if ($PathParts -notcontains $InstallDir) {
        Write-Host "Adding $InstallDir to user PATH..." -ForegroundColor Cyan
        $NewPath = ($PathParts + $InstallDir) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $NewPath, [EnvironmentVariableTarget]::User)
        $env:Path = "$env:Path;$InstallDir"
        Write-Host "Restart your terminal (or run 'refreshenv') to start using 'sn-tui'." -ForegroundColor Yellow
    } else {
        Write-Host "sn-tui is ready! Run 'sn-tui --help' or 'sn-tui' to start." -ForegroundColor Green
    }
} else {
    Write-Error "Failed to install sn-tui: executable not found in release archive."
}
