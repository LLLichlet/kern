param(
    [string]$Version,
    [string]$Target,
    [string]$Archive,
    [string]$Dest,
    [string]$GitHubRepo = "kern-project/kern",
    [switch]$NoPath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$DefaultVersion = "v0.7.6"
$HostTools = @("kernc", "craft", "kern-lsp")

function Fail([string]$Message) {
    throw $Message
}

function Info([string]$Message) {
    Write-Host $Message
}

function Format-ByteSize([Int64]$Bytes) {
    $units = @("B", "KiB", "MiB", "GiB", "TiB")
    $size = [double]$Bytes
    $index = 0
    while ($size -ge 1024 -and $index -lt ($units.Count - 1)) {
        $size /= 1024
        $index += 1
    }

    if ($index -eq 0) {
        return "$([Int64]$size) $($units[$index])"
    }

    return ("{0:N1} {1}" -f $size, $units[$index])
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

function Download-WithCurl([string]$Url, [string]$Destination) {
    $curl = Get-Command "curl.exe" -ErrorAction SilentlyContinue
    if ($null -eq $curl) {
        return $false
    }

    Info "=> Using curl.exe for the SDK download..."
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

    Info "=> Using BITS for the SDK download..."
    try {
        Start-BitsTransfer -Source $Url -Destination $Destination -DisplayName "Kern SDK" -Description "Downloading Kern SDK release archive"
        return (Test-Path $Destination -PathType Leaf)
    } catch {
        Remove-Item -Force $Destination -ErrorAction SilentlyContinue
        return $false
    }
}

function Download-WithWebRequest([string]$Url, [string]$Destination) {
    Info "=> Falling back to Invoke-WebRequest for the SDK download..."
    Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing
    return (Test-Path $Destination -PathType Leaf)
}

function Download-ReleaseArchive([string]$Repo, [string]$ResolvedVersion, [string]$ArchiveName, [string]$Destination) {
    $url = "https://github.com/$Repo/releases/download/$ResolvedVersion/$ArchiveName"
    Info "=> Downloading Kern $ResolvedVersion..."
    Info "=> The Windows SDK archive includes the bundled LLVM/Clang runtime tools needed by Kern."

    try {
        $downloaded = (Download-WithCurl $url $Destination)
        if (-not $downloaded) {
            $downloaded = (Download-WithBits $url $Destination)
        }
        if (-not $downloaded) {
            $downloaded = (Download-WithWebRequest $url $Destination)
        }

        if (-not $downloaded) {
            Fail "download did not produce ``$Destination``"
        }

        $size = (Get-Item -LiteralPath $Destination).Length
        Info "=> Downloaded $(Format-ByteSize $size) into $Destination"
        Info "=> Tip: if you want repeated installs or a fully offline setup, download the release zip once and rerun install.ps1 -Archive <path>."
    } catch {
        Fail "download failed for ``$url``: $($_.Exception.Message)"
    }
}

function Extract-ArchiveRoot([string]$ArchivePath, [string]$ExtractRoot) {
    Info "=> Extracting toolchain..."
    try {
        Remove-Item -Recurse -Force $ExtractRoot -ErrorAction SilentlyContinue
        Add-Type -AssemblyName "System.IO.Compression.FileSystem"
        [System.IO.Compression.ZipFile]::ExtractToDirectory($ArchivePath, $ExtractRoot)
    } catch {
        Remove-Item -Recurse -Force $ExtractRoot -ErrorAction SilentlyContinue
        New-Item -ItemType Directory -Force -Path $ExtractRoot | Out-Null
        Expand-Archive -Path $ArchivePath -DestinationPath $ExtractRoot -Force
    }

    $roots = @(Get-ChildItem -Path $ExtractRoot -Directory)
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

    Validate-ManifestToolchain $SdkRoot $manifest
}

function Get-JsonPropertyValue([object]$Object, [string]$Name) {
    if ($null -eq $Object) {
        return $null
    }

    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }

    return $property.Value
}

function Validate-ManifestToolchain([string]$SdkRoot, [object]$Manifest) {
    $toolchain = Get-JsonPropertyValue $Manifest "toolchain"
    if ($null -eq $toolchain) {
        Fail "SDK manifest is missing the ``toolchain`` section"
    }

    $components = Get-JsonPropertyValue $toolchain "components"
    if ($null -eq $components) {
        Fail "SDK manifest toolchain components are invalid"
    }

    $bundled = [bool](Get-JsonPropertyValue $toolchain "bundled")
    if (-not $bundled) {
        return
    }

    $required = @("clang", "lld")
    if ($Manifest.host_target -like "*windows-msvc") {
        $required += "llvm_lib"
    }

    foreach ($component in $required) {
        $entry = Get-JsonPropertyValue $components $component
        if ($null -eq $entry) {
            Fail "SDK manifest is missing bundled component ``$component``"
        }
    }

    foreach ($property in $components.PSObject.Properties) {
        Validate-ComponentRecord $SdkRoot $property.Name $property.Value
    }

    if ($Manifest.host_target -like "*windows-msvc") {
        foreach ($component in $required) {
            $entry = Get-JsonPropertyValue $components $component
            Verify-WindowsToolchainComponentStarts $SdkRoot $component $entry
        }
    }
}

function Validate-ComponentRecord([string]$SdkRoot, [string]$Component, [object]$Entry) {
    $relativePath = Get-JsonPropertyValue $Entry "path"
    if (-not $relativePath) {
        Fail "SDK manifest component ``$Component`` has no path"
    }

    $kind = Get-JsonPropertyValue $Entry "kind"
    if (-not $kind) {
        $kind = "file"
    }

    $target = Join-Path $SdkRoot $relativePath
    if ($kind -eq "directory") {
        if (-not (Test-Path $target -PathType Container)) {
            Fail "SDK bundled component ``$Component`` is missing at ``$target``"
        }
        return
    }

    if (-not (Test-Path $target -PathType Leaf)) {
        Fail "SDK bundled component ``$Component`` is missing at ``$target``"
    }

    $expectedSize = Get-JsonPropertyValue $Entry "size"
    if ($null -ne $expectedSize) {
        $actualSize = (Get-Item -LiteralPath $target).Length
        if ([Int64]$expectedSize -ne $actualSize) {
            Fail "SDK bundled component ``$Component`` size mismatch at ``$target``"
        }
    }

    $expectedSha = Get-JsonPropertyValue $Entry "sha256"
    if ($expectedSha) {
        $actualSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $target).Hash.ToLowerInvariant()
        if ($actualSha -ne ([string]$expectedSha).ToLowerInvariant()) {
            Fail "SDK bundled component ``$Component`` checksum mismatch at ``$target``"
        }
    }
}

function Verify-WindowsToolchainComponentStarts([string]$SdkRoot, [string]$Component, [object]$Entry) {
    $relativePath = Get-JsonPropertyValue $Entry "path"
    $target = Join-Path $SdkRoot $relativePath
    if (-not (Test-Path $target -PathType Leaf)) {
        Fail "SDK bundled component ``$Component`` is missing at ``$target``"
    }

    if ($Component -eq "llvm_lib") {
        $probeDir = Join-Path ([System.IO.Path]::GetTempPath()) ("kern-llvm-lib-probe-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Force -Path $probeDir | Out-Null
        try {
            $probeOutput = Join-Path $probeDir "empty.lib"
            $output = & $target /llvmlibempty "/out:$probeOutput" 2>&1
            if ($LASTEXITCODE -eq 0) {
                return
            }
        } finally {
            Remove-Item -Recurse -Force $probeDir -ErrorAction SilentlyContinue
        }
    } else {
        $output = & $target --version 2>&1
        if ($LASTEXITCODE -eq 0) {
            return
        }
    }

    $message = ($output | Out-String).Trim()
    Fail "SDK bundled Windows runtime component ``$Component`` failed to start at ``$target``:`n$message"
}

function Copy-SdkContents([string]$SdkRoot, [string]$InstallRoot) {
    Info "=> Installing SDK into $InstallRoot..."
    $resolvedInstallRoot = [System.IO.Path]::GetFullPath($InstallRoot)
    $installParent = Split-Path -Parent $resolvedInstallRoot
    $installName = Split-Path -Leaf $resolvedInstallRoot
    $stagingRoot = Join-Path $installParent (".$installName.installing." + [guid]::NewGuid().ToString("N"))
    $backupRoot = Join-Path $installParent (".$installName.previous." + [guid]::NewGuid().ToString("N"))

    New-Item -ItemType Directory -Force -Path $installParent | Out-Null
    Remove-Item -Recurse -Force $stagingRoot, $backupRoot -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $stagingRoot | Out-Null

    try {
        foreach ($child in Get-ChildItem -Path $SdkRoot) {
            Copy-Item -Recurse -Force $child.FullName -Destination $stagingRoot
        }

        if (Test-Path $resolvedInstallRoot) {
            Move-Item -Force $resolvedInstallRoot $backupRoot
        }

        try {
            Move-Item -Force $stagingRoot $resolvedInstallRoot
        } catch {
            if (Test-Path $backupRoot) {
                Move-Item -Force $backupRoot $resolvedInstallRoot
            }
            throw
        }

        Remove-Item -Recurse -Force $backupRoot -ErrorAction SilentlyContinue
    } catch {
        Remove-Item -Recurse -Force $stagingRoot -ErrorAction SilentlyContinue
        Fail "failed to replace existing installation at ``$resolvedInstallRoot``: $($_.Exception.Message)"
    }
}

function Verify-Binary([string]$BinaryPath, [string]$ResolvedTarget) {
    if (-not (Test-Path $BinaryPath -PathType Leaf)) {
        Fail "installed binary ``$BinaryPath`` is missing"
    }

    $output = & $BinaryPath --version 2>&1
    if ($LASTEXITCODE -eq 0) {
        $versionText = ($output | Out-String).Trim()
        Info "=> Verified $([System.IO.Path]::GetFileName($BinaryPath)): $versionText"
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
