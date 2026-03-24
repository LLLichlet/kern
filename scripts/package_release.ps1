param (
    [string]$Version = "dev",
    [string]$Target = "x86_64-windows-msvc"
)

$DistName = "kern-$Version-$Target"
$ZipFile = "$DistName.zip"

Write-Host "Packaging $DistName..."

New-Item -ItemType Directory -Force -Path "$DistName\bin" | Out-Null
New-Item -ItemType Directory -Force -Path "$DistName\lib\kern" | Out-Null

Copy-Item "target\release\kernc.exe" -Destination "$DistName\bin\"
Copy-Item -Recurse "library\std" -Destination "$DistName\lib\kern\"
Copy-Item "README.md", "LICENSE" -Destination "$DistName\"

Compress-Archive -Path "$DistName\*" -DestinationPath $ZipFile -Force

Write-Host "Successfully packaged: $ZipFile"