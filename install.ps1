param(
    [string]$Version,
    [string]$Target,
    [string]$Kernup,
    [string]$Archive,
    [string]$Dest,
    [string]$GitHubRepo = "kern-project/kern",
    [switch]$NoPath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$DefaultVersion = "v0.8.1"
$VersionSpecified = $PSBoundParameters.ContainsKey("Version")

function Fail([string]$Message) {
    throw $Message
}

function Info([string]$Message) {
    Write-Host $Message
}

function Get-HostTarget {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ($arch -ne [System.Runtime.InteropServices.Architecture]::X64) {
        Fail "Windows installation currently only supports x86_64-windows-msvc"
    }
    return "x86_64-windows-msvc"
}

function Fetch-LatestVersion([string]$Repo) {
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
        if ($release.tag_name) {
            return [string]$release.tag_name
        }
    } catch {
        return $null
    }
    return $null
}

function Download-WithCurl([string]$Url, [string]$Destination) {
    $curl = Get-Command "curl.exe" -ErrorAction SilentlyContinue
    if ($null -eq $curl) {
        return $false
    }

    & $curl.Source --fail --location --retry 5 --retry-delay 2 --retry-all-errors --progress-bar --output $Destination $Url
    if ($LASTEXITCODE -eq 0 -and (Test-Path $Destination -PathType Leaf)) {
        return $true
    }

    Remove-Item -Force $Destination -ErrorAction SilentlyContinue
    return $false
}

function Download-WithBits([string]$Url, [string]$Destination) {
    $bits = Get-Command "Start-BitsTransfer" -ErrorAction SilentlyContinue
    if ($null -eq $bits) {
        return $false
    }

    try {
        Start-BitsTransfer -Source $Url -Destination $Destination -DisplayName "Kern installer" -Description "Downloading Kern installer"
        return (Test-Path $Destination -PathType Leaf)
    } catch {
        Remove-Item -Force $Destination -ErrorAction SilentlyContinue
        return $false
    }
}

function Download-File([string]$Url, [string]$Destination) {
    Info "=> Downloading $Url"
    try {
        $downloaded = (Download-WithCurl $Url $Destination)
        if (-not $downloaded) {
            $downloaded = (Download-WithBits $Url $Destination)
        }
        if (-not $downloaded) {
            Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing
            $downloaded = (Test-Path $Destination -PathType Leaf)
        }
        if (-not $downloaded) {
            Fail "download did not produce ``$Destination``"
        }
    } catch {
        Fail "download failed for ``$Url``: $($_.Exception.Message)"
    }
}

function Extract-ArchiveRoot([string]$ArchivePath, [string]$ExtractRoot) {
    Remove-Item -Recurse -Force $ExtractRoot -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $ExtractRoot | Out-Null

    try {
        Add-Type -AssemblyName "System.IO.Compression.FileSystem"
        [System.IO.Compression.ZipFile]::ExtractToDirectory($ArchivePath, $ExtractRoot)
    } catch {
        Remove-Item -Recurse -Force $ExtractRoot -ErrorAction SilentlyContinue
        New-Item -ItemType Directory -Force -Path $ExtractRoot | Out-Null
        Expand-Archive -Path $ArchivePath -DestinationPath $ExtractRoot -Force
    }

    $roots = @(Get-ChildItem -Path $ExtractRoot -Directory)
    if ($roots.Count -ne 1) {
        Fail "expected exactly one root in ``$ArchivePath``"
    }
    return $roots[0].FullName
}

$HostTarget = Get-HostTarget
if (-not $Target) {
    $Target = $HostTarget
}
if ($Target -ne $HostTarget) {
    Fail "target ``$Target`` does not match the current host ``$HostTarget``"
}

$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("kern-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null

try {
    if ($Kernup) {
        if (-not (Test-Path $Kernup -PathType Leaf)) {
            Fail "kernup binary ``$Kernup`` does not exist"
        }
        $KernupBin = $Kernup
    } else {
        if (-not $Version) {
            $Version = Fetch-LatestVersion $GitHubRepo
        }
        if (-not $Version) {
            $Version = $DefaultVersion
        }

        $KernupArchive = "kernup-$Version-$Target.zip"
        $KernupArchivePath = Join-Path $TempRoot $KernupArchive
        $KernupUrl = "https://github.com/$GitHubRepo/releases/download/$Version/$KernupArchive"
        Info "=> Downloading Kern installer $Version..."
        Download-File $KernupUrl $KernupArchivePath

        $KernupRoot = Extract-ArchiveRoot $KernupArchivePath (Join-Path $TempRoot "kernup")
        $KernupBin = Join-Path $KernupRoot "kernup.exe"
        if (-not (Test-Path $KernupBin -PathType Leaf)) {
            Fail "kernup.exe is missing from ``$KernupArchive``"
        }
    }

    $InstallArgs = @("install", "--target", $Target, "--github-repo", $GitHubRepo)
    if ($VersionSpecified -or (-not $Archive)) {
        if ($Version) {
            $InstallArgs += @("--version", $Version)
        }
    }
    if ($Archive) {
        $InstallArgs += @("--archive", $Archive)
    }
    if ($Dest) {
        $InstallArgs += @("--dest", $Dest)
    }
    if ($NoPath) {
        $InstallArgs += "--no-path"
    }

    & $KernupBin @InstallArgs
    if ($LASTEXITCODE -ne 0) {
        Fail "kernup install failed with exit code $LASTEXITCODE"
    }
} finally {
    Remove-Item -Recurse -Force $TempRoot -ErrorAction SilentlyContinue
}
