$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$DefaultVersion = "v0.7.0"

function Write-Info {
    param([string]$Message)
    Write-Host $Message
}

function Write-WarnMessage {
    param([string]$Message)
    Write-Host "Warning: $Message" -ForegroundColor Yellow
}

function Fail-Install {
    param([string]$Message)
    Write-Host "Error: $Message" -ForegroundColor Red
    exit 1
}

function Get-LatestVersion {
    $ApiUrl = "https://api.github.com/repos/softfault/kern/releases/latest"
    try {
        $ReleaseInfo = Invoke-RestMethod -Uri $ApiUrl
        if ($ReleaseInfo.tag_name) {
            return $ReleaseInfo.tag_name
        }
    } catch {
    }

    return $null
}

function Test-KernBinary {
    param(
        [string]$BinaryName,
        [string]$BinaryPath,
        [string]$KernHome
    )

    if (-not (Test-Path $BinaryPath -PathType Leaf)) {
        Fail-Install "Installed binary $BinaryPath is missing."
    }

    try {
        $Output = & $BinaryPath --version 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "exit code $LASTEXITCODE`n$($Output | Out-String)"
        }
        $TrimmedOutput = ($Output | Out-String).Trim()
        Write-Info "=> Verified $BinaryName: $TrimmedOutput"
    } catch {
        Write-WarnMessage "Failed to start $BinaryName after installation."
        if ($_.Exception.Message) {
            Write-Host $_.Exception.Message -ForegroundColor Yellow
        }
        Write-WarnMessage "Official Windows archives are built with static CRT and should not need the VC++ redistributable."
        Write-WarnMessage "If this still fails, confirm you used the official release archive and that local security policy did not block files under $KernHome."
        Write-WarnMessage "Installed files remain in $KernHome, but the toolchain is not ready to use yet."
        exit 1
    }
}

Write-Host "Welcome to the Kern Programming Language Installer!" -ForegroundColor Cyan
Write-Info "=> Fetching latest version info from GitHub..."

$Version = Get-LatestVersion
if (-not $Version) {
    Write-WarnMessage "Failed to fetch the latest version. Falling back to $DefaultVersion."
    $Version = $DefaultVersion
}

$Target = "x86_64-windows-msvc"
$DistName = "kern-$Version-$Target"
$ZipFile = "$DistName.zip"
$DownloadUrl = "https://github.com/softfault/kern/releases/download/$Version/$ZipFile"

$KernHome = Join-Path $env:USERPROFILE ".kern"
$KernBin = Join-Path $KernHome "bin"

Write-Info "=> Preparing to install Kern $Version toolchain for $Target..."
Write-Info "=> Creating installation directory at $KernHome..."
New-Item -ItemType Directory -Force -Path $KernHome | Out-Null

$TempRoot = Join-Path $env:TEMP ("kern-install-" + [System.Guid]::NewGuid().ToString("N"))
$TempZip = Join-Path $TempRoot $ZipFile
$ExtractPath = Join-Path $TempRoot "extract"
New-Item -ItemType Directory -Force -Path $ExtractPath | Out-Null

try {
    Write-Info "=> Downloading Kern $Version toolchain for Windows..."
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $TempZip

    Write-Info "=> Extracting toolchain..."
    Expand-Archive -Path $TempZip -DestinationPath $ExtractPath -Force

    $ExpandedRoot = Join-Path $ExtractPath $DistName
    if (Test-Path $ExpandedRoot) {
        Copy-Item -Recurse -Force "$ExpandedRoot\*" -Destination $KernHome
    } else {
        Copy-Item -Recurse -Force "$ExtractPath\*" -Destination $KernHome
    }
} finally {
    if (Test-Path $TempRoot) {
        Remove-Item -Recurse -Force $TempRoot
    }
}

Write-Info "=> Verifying installed tools..."
Test-KernBinary -BinaryName "kernc.exe" -BinaryPath (Join-Path $KernBin "kernc.exe") -KernHome $KernHome
Test-KernBinary -BinaryName "craft.exe" -BinaryPath (Join-Path $KernBin "craft.exe") -KernHome $KernHome
Test-KernBinary -BinaryName "kern-lsp.exe" -BinaryPath (Join-Path $KernBin "kern-lsp.exe") -KernHome $KernHome

Write-Info "=> Configuring PATH environment variable..."
$UserPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
if ([string]::IsNullOrWhiteSpace($UserPath)) {
    $NewPath = $KernBin
} else {
    $NewPath = $UserPath.TrimEnd(';') + ";$KernBin"
}

$HasPathEntry = -not [string]::IsNullOrWhiteSpace($UserPath) -and $UserPath -match [regex]::Escape($KernBin)
if (-not $HasPathEntry) {
    [System.Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Host "Added $KernBin to your PATH." -ForegroundColor Green
    Write-Host "Please restart your PowerShell or IDE to apply changes." -ForegroundColor Yellow
} else {
    Write-Host "$KernBin is already in your PATH."
}

Write-Host "`nKern $Version toolchain installed successfully!" -ForegroundColor Green
Write-Host "Run 'kernc --version', 'craft --version', and 'kern-lsp --version' to verify."
