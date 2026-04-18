# Windows Distribution Guide

This document describes the Windows host-tool distribution policy for the
current 0.7.0 toolchain.

It keeps three concerns separate:

- Kern program semantics
- Rust host-tool build/distribution policy
- Windows OS ABI realities

If those layers are blurred together, Windows packaging becomes easy to misread
and easy to break.

## Scope

This document is about the shipped Windows host tools:

- `kernc`
- `craft`
- `kern-lsp`

It is **not** a document about the runtime semantics of a compiled Kern program.

In particular:

- `runtime_entry` describes the compiled Kern program's startup contract
- `runtime_libc` describes whether the compiled Kern program links libc
- neither axis decides how the Rust host tools themselves are distributed

## Canonical Windows Release Policy

Official Windows release archives must satisfy all of the following:

- build the host tools for the real Cargo target triple `x86_64-pc-windows-msvc`
- label the release archive as `x86_64-windows-msvc`
- package binaries from `target/x86_64-pc-windows-msvc/release/`
- bundle the validated host LLVM/Clang toolchain under `toolchain/host/`
- build the host tools with `-C target-feature=+crt-static`

This policy exists because a plain Rust/MSVC release build can depend on:

- `VCRUNTIME140.dll`
- `VCRUNTIME140_1.dll`
- `api-ms-win-crt-*`

That dependency set is not suitable for an official Windows archive because a
clean user machine may fail before the tool even starts. The SDK should carry
its own host LLVM/Clang toolchain instead of expecting users to provision it.

In practice:

- local development may use ordinary `cargo build --release`
- official Windows distribution must use static CRT

## What Static CRT Solves

Static CRT for the host tools removes the need for the VC++ redistributable and
the dynamic UCRT import set in the shipped binaries.

It does **not** make the host tools freestanding in the bare-metal sense.

The shipped Windows binaries are still normal user processes and still import
ordinary system DLLs such as:

- `KERNEL32.dll`
- `ntdll.dll`
- `ADVAPI32.dll`
- `SHELL32.dll`
- `ole32.dll`
- `bcryptprimitives.dll`
- `api-ms-win-core-synch-l1-2-0.dll`

Those are Windows OS ABI dependencies for the host tools themselves. They are
not hidden libc baggage in Kern's language model.

## Canonical Build Commands

### Local Development Build

For local development on Windows:

```powershell
cargo build --release
```

This produces binaries under:

```text
target/release/
```

This is valid for local work, but it is not the authoritative release path.

### Official-Style Windows Release Build

For release-quality Windows host-tool binaries:

```powershell
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release --target x86_64-pc-windows-msvc -p kernc_cli --bin kernc
cargo build --release --target x86_64-pc-windows-msvc -p craft
cargo build --release --target x86_64-pc-windows-msvc -p kern-lsp
```

This produces binaries under:

```text
target/x86_64-pc-windows-msvc/release/
```

## Official Packaging Script

The repository Python operations entry point is the canonical Windows packaging
entry point:

```powershell
py -3 -m ops release package --version v0.7.0 --target x86_64-windows-msvc
```

The script currently enforces the important Windows-specific rules:

- it stops on PowerShell errors instead of continuing after a failed copy
- it translates the archive label `x86_64-windows-msvc` to the actual Cargo
  target triple `x86_64-pc-windows-msvc`
- it enables static CRT for Windows host-tool release builds
- it packages binaries from the real target output directory, not from
  `target/release/`

## Installation Model

The user-facing Windows installer is the repository root [install.ps1](../install.ps1)
entrypoint. It should perform installation directly instead of delegating to
repository Python tooling.

It downloads the prebuilt archive and expands it into:

```text
%USERPROFILE%\.kern
```

It then adds:

```text
%USERPROFILE%\.kern\bin
```

to the user PATH.

This means the quality of the release archive matters directly. If the archive
itself is wrong, the installer will still install the wrong thing.

The current Windows SDK archive is intentionally heavy because it includes the
bundled host LLVM/Clang toolchain. That means installer UX matters:

- prefer `curl.exe` or another large-file-capable Windows transport over
  defaulting straight to `Invoke-WebRequest`
- expect first-install download and extraction to take noticeable time on
  slower links or machines with aggressive antivirus scanning
- keep the `-Archive <path>` offline-install path available so one download can
  be reused across repeated installs

The offline-install path should be documented concretely. A correct example is:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Archive .\kern-v0.7.0-x86_64-windows-msvc.zip
```

If the archive filename no longer contains the release tag, users should pass
the version explicitly:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Version v0.7.0 -Archive .\kern.zip
```

The Python `ops` entrypoints remain valid for CI and repository engineering,
but they are not the user-install contract on Windows.

## Common Windows Footguns

### 1. Archive Label Versus Cargo Target Triple

These are **not** the same string:

- archive label: `x86_64-windows-msvc`
- Cargo target triple: `x86_64-pc-windows-msvc`

Using the archive label as the Cargo target triple is wrong.

### 2. `target/release/` Versus `target/<triple>/release/`

These are different output directories:

- local default output: `target/release/`
- explicit target output: `target/x86_64-pc-windows-msvc/release/`

If release packaging is supposed to use static CRT and an explicit target, the
package step must copy from the explicit-target directory.

### 3. Thinking `runtime_libc = no` Means the Host Tools Are DLL-Free

That flag describes the compiled Kern program, not the Rust host tools.

It is completely possible for:

- Kern program policy to remain pure-first
- the shipped Windows host tools to still depend on dynamic CRT

Those are separate layers.

### 4. Thinking Static CRT Means "No Windows Dependencies"

Static CRT removes redistributable baggage. It does not erase the Windows OS
ABI baseline used by the host tools.

If a tool imports `KERNEL32.dll` or `SHELL32.dll`, that is normal host-OS
behavior, not a violation of Kern's pure-first language/runtime model.

### 5. Promising Very Old Windows Versions

Do not overstate compatibility just because the VC++ redistributable dependency
has been removed.

Static CRT solves one class of distribution failure. It does not prove that the
host tools support every old Windows API baseline.

The current safe statement is:

- official host-tool archives target modern Windows systems
- very old Windows versions should not be promised implicitly

## Failure Modes And First Checks

### The Tool Does Not Start On A User Machine

First ask:

- was this built from the official release archive
- or was it built locally with plain `cargo build --release`

If it was a local plain MSVC release build, dynamic CRT dependency is the first
thing to suspect.

### The Package Script Says Success But The Archive Is Wrong

The package script should be the authoritative path. If it reports success but
the archive contents are wrong, verify:

- the script was run from the repository root
- the Windows target triple mapping is still present
- the binaries were copied from `target/x86_64-pc-windows-msvc/release/`
- the static-CRT environment flag was applied for Windows packaging

### The Tool Starts, But Someone Calls The Remaining DLL Imports "Baggage"

Check which DLL class is being discussed:

- `VCRUNTIME140*.dll` / `api-ms-win-crt-*`: redistributable baggage for host-tool distribution
- `KERNEL32.dll` / `ADVAPI32.dll` / `SHELL32.dll` / `ole32.dll`: ordinary Windows system ABI dependencies

These must not be conflated.

## Practical Summary

The practical rules are:

- Kern program runtime semantics and Windows host-tool distribution policy are separate concerns.
- Official Windows archives must use static CRT.
- Official Windows packaging must build for `x86_64-pc-windows-msvc` and package from that target directory.
- Remaining Win32 system DLL imports are normal host-OS ABI dependencies.
- Removing dynamic CRT dependency does not automatically imply support for very old Windows versions.
