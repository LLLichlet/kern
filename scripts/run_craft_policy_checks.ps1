$AllowedFixture = "tools/craft/fixtures/release-policy/allowed"
$AllowedExceptionFixture = "tools/craft/fixtures/release-policy/allowed-exception"
$BlockedFixture = "tools/craft/fixtures/release-policy/blocked"

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
$Output = & cargo run -p craft -- check --release $BlockedFixture 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Error "craft release policy fixture unexpectedly passed: $BlockedFixture"
    exit 1
}
if (-not (($Output | Out-String).Contains("release source policy rejected"))) {
    $Output | Write-Host
    Write-Error "craft release policy fixture failed for an unexpected reason"
    exit 1
}

Write-Host "craft release policy fixtures passed"
