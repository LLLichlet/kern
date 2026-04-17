$ErrorActionPreference = "Stop"

function Print-CommandPath {
    param([string]$Name)
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        Write-Output "${Name}: <missing>"
    } else {
        Write-Output "${Name}: $($cmd.Source)"
    }
}

function Print-FirstLine {
    param(
        [string]$Label,
        [string]$FilePath,
        [string[]]$Arguments = @("--version")
    )

    if (-not (Test-Path $FilePath)) {
        Write-Output "${Label}: <missing>"
        return
    }

    try {
        $output = & $FilePath @Arguments 2>&1 | Select-Object -First 1
        if ($null -eq $output -or [string]::IsNullOrWhiteSpace("$output")) {
            Write-Output "${Label}: <no output>"
        } else {
            Write-Output "${Label}: $output"
        }
    } catch {
        Write-Output "${Label}: <failed>"
    }
}

Write-Output "runner_os: $([System.Runtime.InteropServices.RuntimeInformation]::OSDescription)"
Write-Output "runner_arch: $env:PROCESSOR_ARCHITECTURE"
Write-Output "LLVM_SYS_211_PREFIX: $(if ($env:LLVM_SYS_211_PREFIX) { $env:LLVM_SYS_211_PREFIX } else { '<unset>' })"
Write-Output "CC: $(if ($env:CC) { $env:CC } else { '<unset>' })"
Write-Output "CXX: $(if ($env:CXX) { $env:CXX } else { '<unset>' })"
if ($env:LLVM_SYS_211_PREFIX) {
    Write-Output "prefix clang: $env:LLVM_SYS_211_PREFIX\bin\clang.exe"
    Write-Output "prefix clang++: $env:LLVM_SYS_211_PREFIX\bin\clang++.exe"
    Write-Output "prefix lld-link: $env:LLVM_SYS_211_PREFIX\bin\lld-link.exe"
    Write-Output "prefix llvm-lib: $env:LLVM_SYS_211_PREFIX\bin\llvm-lib.exe"
}

Print-CommandPath clang
Print-CommandPath clang++
Print-CommandPath lld-link
Print-CommandPath link
Print-CommandPath llvm-lib
Print-CommandPath llvm-config

$clang = Get-Command clang -ErrorAction SilentlyContinue
if ($null -ne $clang) {
    Print-FirstLine "clang --version" $clang.Source
}
$clangxx = Get-Command clang++ -ErrorAction SilentlyContinue
if ($null -ne $clangxx) {
    Print-FirstLine "clang++ --version" $clangxx.Source
}
$lld = Get-Command lld-link -ErrorAction SilentlyContinue
if ($null -ne $lld) {
    Print-FirstLine "lld-link --version" $lld.Source
}
$llvmLib = Get-Command llvm-lib -ErrorAction SilentlyContinue
if ($null -ne $llvmLib) {
    Print-FirstLine "llvm-lib --version" $llvmLib.Source
}
$llvmConfig = Get-Command llvm-config -ErrorAction SilentlyContinue
if ($null -ne $llvmConfig) {
    Print-FirstLine "llvm-config --version" $llvmConfig.Source
}
if ($env:LLVM_SYS_211_PREFIX) {
    $prefixLldLink = Join-Path $env:LLVM_SYS_211_PREFIX "bin\lld-link.exe"
    $prefixLlvmLib = Join-Path $env:LLVM_SYS_211_PREFIX "bin\llvm-lib.exe"
    if (Test-Path $prefixLldLink) {
        Print-FirstLine "prefix lld-link --version" $prefixLldLink
    }
    if (Test-Path $prefixLlvmLib) {
        Print-FirstLine "prefix llvm-lib --version" $prefixLlvmLib
    }
}
