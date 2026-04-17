param(
    [string]$Version,
    [string]$Target,
    [string]$Archive,
    [string]$Dest,
    [string]$GitHubRepo = "softfault/kern",
    [switch]$NoPath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$DefaultVersion = "v0.7.0"
$HostTools = @("kernc", "craft", "kern-lsp")

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

function Get-DefaultInstallRoot {
    if ($env:USERPROFILE) {
        return Join-Path $env:USERPROFILE ".kern"
    }
    return Join-Path $HOME ".kern"
}

function Infer-VersionFromArchiveName([string]$Name, [string]$ExpectedTarget) {
    $prefix = "kern-"
    $suffix = "-$ExpectedTarget.zip"
    if ($Name.StartsWith($prefix) -and $Name.EndsWith($suffix)) {
        return $Name.Substring($prefix.Length, $Name.Length - $prefix.Length - $suffix.Length)
    }
    return $null
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

function Download-ReleaseArchive([string]$Repo, [string]$ResolvedVersion, [string]$ArchiveName, [string]$Destination) {
    $url = "https://github.com/$Repo/releases/download/$ResolvedVersion/$ArchiveName"
    Info "=> Downloading Kern $ResolvedVersion..."
    try {
        Invoke-WebRequest -Uri $url -OutFile $Destination
    } catch {
        Fail "download failed for ``$url``: $($_.Exception.Message)"
    }
}

function Extract-ArchiveRoot([string]$ArchivePath, [string]$ExtractRoot) {
    Info "=> Extracting toolchain..."
    New-Item -ItemType Directory -Force -Path $ExtractRoot | Out-Null
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractRoot -Force
    $roots = Get-ChildItem -Path $ExtractRoot -Directory
    if ($roots.Count -ne 1) {
        Fail "expected exactly one SDK root in ``$ArchivePath``"
    }
    return $roots[0].FullName
}

function Validate-SdkRoot([string]$SdkRoot, [string]$ExpectedTarget) {
    $manifestPath = Join-Path $SdkRoot "manifest\sdk.json"
    if (-not (Test-Path $manifestPath -PathType Leaf)) {
        Fail "SDK manifest ``$manifestPath`` is missing"
    }

    $manifest = Get-Content -Raw -Path $manifestPath | ConvertFrom-Json
    if ($manifest.host_target -ne $ExpectedTarget) {
        Fail "SDK host target mismatch in ``$manifestPath``"
    }

    foreach ($binary in $HostTools) {
        $binaryPath = Join-Path $SdkRoot "bin\$binary.exe"
        if (-not (Test-Path $binaryPath -PathType Leaf)) {
            Fail "SDK binary ``$binary`` is missing from ``$SdkRoot``"
        }
    }

    $toolchainBin = Join-Path $SdkRoot "toolchain\host\bin"
    if (-not (Test-Path $toolchainBin -PathType Container)) {
        Fail "SDK toolchain layout is incomplete"
    }
}

function Copy-SdkContents([string]$SdkRoot, [string]$InstallRoot) {
    Info "=> Installing SDK into $InstallRoot..."
    New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null

    foreach ($child in Get-ChildItem -Path $SdkRoot) {
        $destination = Join-Path $InstallRoot $child.Name
        if (Test-Path $destination) {
            Remove-Item -Recurse -Force $destination
        }
        Copy-Item -Recurse -Force $child.FullName -Destination $InstallRoot
    }
}

function Verify-Binary([string]$BinaryPath, [string]$ResolvedTarget) {
    if (-not (Test-Path $BinaryPath -PathType Leaf)) {
        Fail "installed binary ``$BinaryPath`` is missing"
    }

    $output = & $BinaryPath --version 2>&1
    if ($LASTEXITCODE -eq 0) {
        Info "=> Verified $([System.IO.Path]::GetFileName($BinaryPath)): $($output | Out-String).Trim()"
        return
    }

    $message = ($output | Out-String).Trim()
    $message += "`nOfficial Windows archives should be static-CRT. If startup still fails, inspect local security policy and archive provenance."
    Fail "failed to start ``$BinaryPath`` after installation:`n$message"
}

function Configure-Path([string]$InstallBin) {
    Info "=> Configuring PATH..."
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($null -eq $current) {
        $current = ""
    }

    $entries = @()
    if ($current) {
        $entries = $current.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries)
    }

    foreach ($entry in $entries) {
        if ($entry -eq $InstallBin) {
            Info "$InstallBin is already in your PATH."
            return
        }
    }

    $newValue = if ($current) { "$current;$InstallBin" } else { $InstallBin }
    [Environment]::SetEnvironmentVariable("Path", $newValue, "User")
    Info "Added $InstallBin to your user PATH."
}

$HostTarget = Get-HostTarget
if (-not $Target) {
    $Target = $HostTarget
}
if ($Target -ne $HostTarget) {
    Fail "target ``$Target`` does not match the current host ``$HostTarget``"
}

if (-not $Dest) {
    $Dest = Get-DefaultInstallRoot
}

$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("kern-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null

try {
    if ($Archive) {
        $ArchivePath = (Resolve-Path $Archive).Path
        if (-not $Version) {
            $Version = Infer-VersionFromArchiveName ([System.IO.Path]::GetFileName($ArchivePath)) $Target
        }
    } else {
        if (-not $Version) {
            $Version = Fetch-LatestVersion $GitHubRepo
        }
        if (-not $Version) {
            $Version = $DefaultVersion
        }

        $ArchiveName = "kern-$Version-$Target.zip"
        $ArchivePath = Join-Path $TempRoot $ArchiveName
        Download-ReleaseArchive $GitHubRepo $Version $ArchiveName $ArchivePath
    }

    if (-not $Version) {
        Fail "failed to resolve release version"
    }

    $ExtractRoot = Join-Path $TempRoot "extract"
    $SdkRoot = Extract-ArchiveRoot $ArchivePath $ExtractRoot
    Validate-SdkRoot $SdkRoot $Target
    Copy-SdkContents $SdkRoot $Dest

    $InstallBin = Join-Path $Dest "bin"
    Info "=> Verifying installed tools..."
    foreach ($binary in $HostTools) {
        Verify-Binary (Join-Path $InstallBin "$binary.exe") $Target
    }

    if (-not $NoPath) {
        Configure-Path $InstallBin
    }

    Info "Kern $Version toolchain installed successfully!"
} finally {
    Remove-Item -Recurse -Force $TempRoot -ErrorAction SilentlyContinue
}
