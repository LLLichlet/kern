$AllowedFixture = "tools/craft/fixtures/release-policy/allowed"
$AllowedExceptionFixture = "tools/craft/fixtures/release-policy/allowed-exception"
$BlockedFixture = "tools/craft/fixtures/release-policy/blocked"
$WorkspaceManifest = "Cargo.toml"

$WorkspaceManifestLines = Get-Content $WorkspaceManifest
$CurrentKernVersion = $null
$InWorkspacePackage = $false
foreach ($Line in $WorkspaceManifestLines) {
    if ($Line -match '^\[workspace\.package\]') {
        $InWorkspacePackage = $true
        continue
    }
    if ($InWorkspacePackage -and $Line -match '^\[') {
        break
    }
    if ($InWorkspacePackage -and $Line -match '^version = "(.*)"$') {
        $CurrentKernVersion = $Matches[1]
        break
    }
}

if (-not $CurrentKernVersion) {
    Write-Error "failed to resolve current workspace version from $WorkspaceManifest"
    exit 1
}

$TempRoot = Join-Path $env:TEMP ("craft-policy-" + [guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $TempRoot | Out-Null

function Invoke-CargoCapture {
    param(
        [string[]]$Arguments
    )

    $stdoutPath = Join-Path $env:TEMP ("craft-policy-stdout-" + [guid]::NewGuid().ToString() + ".log")
    $stderrPath = Join-Path $env:TEMP ("craft-policy-stderr-" + [guid]::NewGuid().ToString() + ".log")

    try {
        $process = Start-Process -FilePath "cargo" `
            -ArgumentList $Arguments `
            -NoNewWindow `
            -Wait `
            -PassThru `
            -RedirectStandardOutput $stdoutPath `
            -RedirectStandardError $stderrPath

        $stdout = if (Test-Path $stdoutPath) { Get-Content $stdoutPath -Raw } else { "" }
        $stderr = if (Test-Path $stderrPath) { Get-Content $stderrPath -Raw } else { "" }

        return @{
            ExitCode = $process.ExitCode
            Stdout = $stdout
            Stderr = $stderr
            Output = $stdout + $stderr
        }
    }
    finally {
        Remove-Item -Force $stdoutPath -ErrorAction SilentlyContinue
        Remove-Item -Force $stderrPath -ErrorAction SilentlyContinue
    }
}

function Prepare-Fixture {
    param(
        [string]$SourceDir
    )

    $Destination = Join-Path $TempRoot ([System.IO.Path]::GetFileName($SourceDir))
    Copy-Item -Recurse -Force $SourceDir $Destination

    $ManifestPath = Join-Path $Destination "Craft.toml"
    $ManifestSource = Get-Content $ManifestPath -Raw
    $UpdatedManifest = [System.Text.RegularExpressions.Regex]::Replace(
        $ManifestSource,
        '^kern = ".*"$',
        "kern = `"$CurrentKernVersion`"",
        [System.Text.RegularExpressions.RegexOptions]::Multiline
    )
    Set-Content -Path $ManifestPath -Value $UpdatedManifest -NoNewline

    return $Destination
}

$AllowedPath = Prepare-Fixture $AllowedFixture
$AllowedExceptionPath = Prepare-Fixture $AllowedExceptionFixture
$BlockedPath = Prepare-Fixture $BlockedFixture

Write-Host "Running craft release policy allow fixture..."
cargo run -p craft -- check --project-path $AllowedPath --profile release
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "Running craft release policy allow-exception fixture..."
cargo run -p craft -- check --project-path $AllowedExceptionPath --profile release
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "Running craft release policy block fixture..."
$Result = Invoke-CargoCapture -Arguments @("run", "-p", "craft", "--", "check", "--project-path", $BlockedPath, "--profile", "release")
if ($Result.ExitCode -eq 0) {
    Write-Error "craft release policy fixture unexpectedly passed: $BlockedPath"
    exit 1
}
if (-not $Result.Output.Contains("release source policy rejected")) {
    if ($Result.Stdout) {
        Write-Host $Result.Stdout
    }
    if ($Result.Stderr) {
        Write-Host $Result.Stderr
    }
    Write-Error "craft release policy fixture failed for an unexpected reason"
    exit 1
}

Write-Host "craft release policy fixtures passed"
Remove-Item -Recurse -Force $TempRoot -ErrorAction SilentlyContinue
