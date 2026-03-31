param (
    [string]$Version = "dev",
    [string]$Target = "x86_64-windows-msvc",
    [switch]$SkipBuild
)

$DistName = "kern-$Version-$Target"
$ZipFile = "$DistName.zip"

if (-not $SkipBuild) {
    Write-Host "Building release binaries..."
    cargo build --release -p kernc_cli --bin kernc
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    cargo build --release -p craft
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

Copy-Item "target\release\kernc.exe" -Destination "$DistName\bin\"
Copy-Item "target\release\craft.exe" -Destination "$DistName\bin\"
Copy-Item -Recurse "library\std" -Destination "$DistName\lib\kern\"
Copy-Item "README.md", "LICENSE" -Destination "$DistName\"

Compress-Archive -Path "$DistName\*" -DestinationPath $ZipFile -Force

Write-Host "Successfully packaged: $ZipFile"
