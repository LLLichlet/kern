$AllowedFixture = "tools/craft/fixtures/release-policy/allowed"
$AllowedExceptionFixture = "tools/craft/fixtures/release-policy/allowed-exception"
$BlockedFixture = "tools/craft/fixtures/release-policy/blocked"

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

Write-Host "Running craft release policy allow fixture..."
cargo run -p craft -- check --release $AllowedFixture
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "Running craft release policy allow-exception fixture..."
cargo run -p craft -- check --release $AllowedExceptionFixture
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "Running craft release policy block fixture..."
$Result = Invoke-CargoCapture -Arguments @("run", "-p", "craft", "--", "check", "--release", $BlockedFixture)
if ($Result.ExitCode -eq 0) {
    Write-Error "craft release policy fixture unexpectedly passed: $BlockedFixture"
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
