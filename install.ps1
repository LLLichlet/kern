# install.ps1
Write-Host "Welcome to the Kern Programming Language Installer!" -ForegroundColor Cyan

# 获取最新版本号
Write-Host "=> Fetching latest version info from GitHub..."
$ApiUrl = "https://api.github.com/repos/softfault/kern/releases/latest"
try {
    $ReleaseInfo = Invoke-RestMethod -Uri $ApiUrl
    $Version = $ReleaseInfo.tag_name
} catch {
    Write-Host "Warning: Failed to fetch latest version. Falling back to v0.5.2" -ForegroundColor Yellow
    $Version = "v0.5.2"
}

$Target = "x86_64-windows-msvc"
$DistName = "kern-$Version-$Target"
$ZipFile = "$DistName.zip"
$DownloadUrl = "https://github.com/softfault/kern/releases/download/$Version/$ZipFile"

$KernHome = "$env:USERPROFILE\.kern"
$KernBin = "$KernHome\bin"

Write-Host "=> Creating installation directory at $KernHome..."
New-Item -ItemType Directory -Force -Path $KernHome | Out-Null

Write-Host "=> Downloading Kern $Version for Windows..."
$TempZip = "$env:TEMP\$ZipFile"
Invoke-WebRequest -Uri $DownloadUrl -OutFile $TempZip

Write-Host "=> Extracting toolchain..."
$ExtractPath = "$env:TEMP\$DistName"

Expand-Archive -Path $TempZip -DestinationPath $ExtractPath -Force
if (Test-Path "$ExtractPath\$DistName") {
    Copy-Item -Recurse -Force "$ExtractPath\$DistName\*" -Destination $KernHome
} else {
    Copy-Item -Recurse -Force "$ExtractPath\*" -Destination $KernHome
}
Remove-Item -Force $TempZip
Remove-Item -Recurse -Force $ExtractPath

Write-Host "=> Configuring PATH environment variable..."
$UserPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notmatch [regex]::Escape($KernBin)) {
    $NewPath = $UserPath + ";$KernBin"
    [System.Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Host "Added $KernBin to your PATH." -ForegroundColor Green
    Write-Host "Please restart your PowerShell or IDE to apply changes." -ForegroundColor Yellow
} else {
    Write-Host "$KernBin is already in your PATH."
}

Write-Host "`nKern $Version installed successfully!" -ForegroundColor Green
Write-Host "Run 'kernc --version' to verify."