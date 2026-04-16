param (
    [string]$Version = "dev",
    [string]$Target = "x86_64-windows-msvc",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

if ($args -contains "-h" -or $args -contains "--help") {
    Write-Host @"
Usage:
  scripts/package_release.ps1 [-Version <label>] [-Target <triple>] [-SkipBuild]

Arguments:
  -Version    Archive version label, defaults to "dev"
  -Target     Target triple label in the archive name
  -SkipBuild  Reuse existing release binaries instead of rebuilding
"@
    exit 0
}

$DistName = "kern-$Version-$Target"
$ZipFile = "$DistName.zip"
$CargoTarget = switch ($Target) {
    "x86_64-windows-msvc" { "x86_64-pc-windows-msvc" }
    default { $Target }
}
$BuildArgs = @("--release", "--target", $CargoTarget)
$BinaryDir = Join-Path "target" "$CargoTarget\release"
$StaticCrtVar = $null

if ($Target -eq "x86_64-windows-msvc") {
    # Ship Windows release binaries without requiring the VC++ redistributable.
    $StaticCrtVar = "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS"
    Set-Item -Path "Env:$StaticCrtVar" -Value "-C target-feature=+crt-static"
}

if (-not $SkipBuild) {
    Write-Host "Building release binaries..."
    cargo build @BuildArgs -p kernc_cli --bin kernc
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    cargo build @BuildArgs -p craft
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    cargo build @BuildArgs -p kern-lsp
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

Write-Host "Packaging $DistName..."

if (Test-Path $DistName) {
    Remove-Item -Recurse -Force $DistName
}
if (Test-Path $ZipFile) {
    Remove-Item -Force $ZipFile
}

New-Item -ItemType Directory -Force -Path "$DistName\bin" | Out-Null
New-Item -ItemType Directory -Force -Path "$DistName\lib\kern" | Out-Null

Copy-Item (Join-Path $BinaryDir "kernc.exe") -Destination "$DistName\bin\"
Copy-Item (Join-Path $BinaryDir "craft.exe") -Destination "$DistName\bin\"
Copy-Item (Join-Path $BinaryDir "kern-lsp.exe") -Destination "$DistName\bin\"
Copy-Item -Recurse "library\base" -Destination "$DistName\lib\kern\"
Copy-Item -Recurse "library\rt" -Destination "$DistName\lib\kern\"
Copy-Item -Recurse "library\sys" -Destination "$DistName\lib\kern\"
Copy-Item -Recurse "library\std" -Destination "$DistName\lib\kern\"
Copy-Item "README.md", "LICENSE" -Destination "$DistName\"

Compress-Archive -Path "$DistName\*" -DestinationPath $ZipFile -Force

if ($StaticCrtVar) {
    Remove-Item Env:$StaticCrtVar -ErrorAction SilentlyContinue
}

Write-Host "Successfully packaged: $ZipFile"
