$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$LocalInstaller = Join-Path $Root "ops\install.py"
$RemoteInstallerUrl = "https://raw.githubusercontent.com/softfault/kern/main/ops/install.py"

$Python = Get-Command py -ErrorAction SilentlyContinue
$PythonArgs = @()
if ($null -ne $Python) {
    $PythonExecutable = $Python.Source
    $PythonArgs += "-3"
} else {
    $Python = Get-Command python -ErrorAction SilentlyContinue
    if ($null -eq $Python) {
        Write-Host "Error: neither `py` nor `python` was found in PATH." -ForegroundColor Red
        exit 1
    }
    $PythonExecutable = $Python.Source
}

if (Test-Path $LocalInstaller) {
    & $PythonExecutable @PythonArgs $LocalInstaller @args
    exit $LASTEXITCODE
}

$TempInstaller = Join-Path $env:TEMP ("kern-install-" + [guid]::NewGuid().ToString("N") + ".py")
try {
    Invoke-WebRequest -Uri $RemoteInstallerUrl -OutFile $TempInstaller
    & $PythonExecutable @PythonArgs $TempInstaller @args
    exit $LASTEXITCODE
} finally {
    Remove-Item -Force $TempInstaller -ErrorAction SilentlyContinue
}
