param(
    [ValidateSet("smoke", "hosted", "all")]
    [string]$Mode = "all"
)

$SmokeTests = @(
    "anonymous_aggregates",
    "atomics",
    "regressions",
    "stdlib",
    "traits"
)

$HostedTests = @(
    "collections",
    "filesystem"
)

function Run-TestGroup {
    param(
        [string]$Label,
        [string[]]$Tests
    )

    Write-Host "Running $Label suite..."
    foreach ($TestName in $Tests) {
        cargo test -p kernc_cli --test $TestName
        if ($LASTEXITCODE -ne 0) {
            exit $LASTEXITCODE
        }
    }
}

switch ($Mode) {
    "smoke" {
        Run-TestGroup -Label "smoke" -Tests $SmokeTests
    }
    "hosted" {
        Run-TestGroup -Label "hosted" -Tests $HostedTests
    }
    "all" {
        Run-TestGroup -Label "smoke" -Tests $SmokeTests
        Run-TestGroup -Label "hosted" -Tests $HostedTests
    }
}
