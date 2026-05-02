# Unix Distribution Guide

This document describes the Linux and macOS host-tool distribution policy for
the current 0.7.3 toolchain.

It keeps three concerns separate:

- Kern program semantics
- Rust host-tool build/distribution policy
- Unix host ABI realities

If those layers are blurred together, Unix packaging becomes easy to mislabel,
easy to over-promise, and hard to debug on a clean user machine.

## Scope

This document is about the shipped Unix host tools:

- `kernc`
- `craft`
- `kern-lsp`

It is **not** a document about the runtime semantics of a compiled Kern program.

In particular:

- `runtime_entry` describes the compiled Kern program's startup contract
- `runtime_libc` describes whether the compiled Kern program links libc
- neither axis decides how the Rust host tools themselves are distributed

## Canonical Unix Release Policy

Official Linux/macOS release archives must satisfy all of the following:

- package only host-native targets that the build machine actually matches
- label the archive with that real host target
- package binaries that were built for that same host target
- bundle a runtime-complete host LLVM/Clang subset under `toolchain/host/`
- verify `kernc`, `craft`, and `kern-lsp` start successfully after installation
- avoid promising that current Unix archives are fully static
- avoid promising that one Unix archive runs on every distro or every historical
  OS release

This policy exists because current Unix host-tool binaries still inherit runtime
dependencies from the Rust + C++ host stack, while the SDK now also carries a
bundled runtime-complete LLVM/Clang subset for compile/link stability.

The full LLVM development prefix remains a separate concern and continues to
live in the standalone `package-toolchain` artifact rather than the default
end-user SDK.

Today that means a clean user machine can still fail because of:

- missing shared libraries such as `libstdc++`, `zlib`, or `zstd`
- an older `glibc` baseline than the release archive was built against
- missing or incompatible host OS libc / SDK pieces outside the bundled
  `toolchain/host/` payload
- local macOS policy or loader behavior outside the scope of "just unzip it"

In practice:

- local development may use ordinary `cargo build --release`
- official Unix distribution must keep the archive label honest
- official Unix installers must verify that the installed tools actually start

## What This Policy Solves

This policy does **not** claim that Unix host tools are freestanding in the
bare-metal sense.

It solves a different class of problems:

- avoiding archive labels that do not match the real built binaries
- refusing to call installation "successful" before the tools have been started
- making Unix failure modes diagnosable instead of silent or misleading

It does **not** mean:

- Linux archives are fully static
- Linux archives are automatically portable across every distro baseline
- macOS archives have no dependency on system-provided user-process facilities

## Current Unix Host Reality

The current Linux host tools are not fully static.

In the current tree, `kernc` and `craft` depend on host-side libraries from the
Rust/LLVM/C++ stack, and `kern-lsp` still depends on the normal host C/C++
runtime surface.

That matters for distribution because "the installer copied files into
`~/.kern`" is not enough. The installed binaries must be executed at least once
to prove that the user machine can actually start them.

For macOS, fully static host-tool distribution is not the right mental model in
the first place. The correct policy is bounded host support plus immediate
installer verification.

## Current Official Release Baseline

The official release workflow should define the baseline explicitly instead of
delegating it to moving labels such as `ubuntu-latest`.

The current intended release baseline is:

- Linux `x86_64-linux-gnu`: built on `ubuntu-24.04`
- macOS `x86_64-apple-darwin`: built on `macos-15-intel`
- macOS `aarch64-apple-darwin`: built on `macos-14`

That does **not** mean the archive only runs on those exact OS versions.

It means:

- those runner images define the current official host packaging baseline
- compatibility promises should not be stated more broadly than that baseline
  justifies
- if the project wants broader Linux compatibility later, the correct move is to
  intentionally shift the build baseline, not to quietly rely on whatever
  `ubuntu-latest` happens to mean that month

## Canonical Build And Packaging Commands

### Local Development Build

For local development on Linux or macOS:

```bash
cargo build --release
```

This produces binaries under:

```text
target/release/
```

This is valid for local work, but it is not a statement about universal Unix
distribution compatibility.

### Official-Style Unix Release Build

For release-quality host-native Unix archives:

```bash
python3 -m ops release package --version v0.7.3 --target <host-target>
```

Examples:

```bash
python3 -m ops release package --version v0.7.3 --target x86_64-linux-gnu
python3 -m ops release package --version v0.7.3 --target x86_64-apple-darwin
python3 -m ops release package --version v0.7.3 --target aarch64-apple-darwin
```

The important policy point is that `<host-target>` is a host label, not a free
cross-compilation selector. The current Unix packaging script is intentionally
host-native and must reject mismatched labels.

## Official Packaging Script

The repository Python operations entry point is the canonical packaging entry
point:

```bash
python3 -m ops release package --version v0.7.3 --target <host-target>
```

The script should enforce the important Unix-specific rules:

- it must run from the repository root
- it must only package a target label that matches the current host machine
- it must package binaries that were actually built for that host machine
- it must not claim that a mislabeled archive is a valid release artifact

## Installation Model

The user-facing Unix installer is the repository root [install.sh](../install.sh)
entrypoint. It should perform installation directly instead of trampolining into
repository Python tooling.

It downloads the prebuilt archive and expands it into:

```text
~/.kern
```

It then adds:

```text
~/.kern/bin
```

to the user PATH.

The important rule is that extraction alone is not enough. The installer must
run:

- `kernc --version`
- `craft --version`
- `kern-lsp --version`

before it claims success.

The Python `ops` entrypoints remain valid for CI and repository engineering,
but they are not the user-install contract on Unix.

If startup fails, the installer should stop and print the most likely host-side
remediation instead of silently leaving the user with a broken installation.

## Common Unix Footguns

### 1. Treating The Archive Label As A Cross-Compilation Knob

On Unix, the archive label is a release identity, not proof that cross-target
host-tool packaging was performed correctly.

If a script copies from `target/release/`, then labeling the archive as some
other target without a matching build is wrong.

### 2. Thinking `runtime_libc = no` Means The Host Tools Are Fully Static

That flag describes the compiled Kern program, not the Rust host tools.

It is completely possible for:

- Kern program policy to remain pure-first
- the shipped Unix host tools to still depend on host libraries

Those are separate layers.

### 3. Promising "Linux" Without A Baseline

"Linux" is not one uniform runtime environment.

Do not imply that one `x86_64-linux-gnu` archive automatically covers:

- every glibc version
- every distro release age
- every machine missing `libstdc++`, `zlib`, or `zstd`

The safe statement is:

- official Linux archives target the bounded userland implied by the pinned
  release build host, currently `ubuntu-24.04`
- older or more minimal systems may need extra runtime libraries or a local
  source build

### 4. Treating macOS Like A Static-Binary Problem

For macOS, the useful questions are:

- what minimum OS baseline is being targeted
- whether the installed tool starts on that host
- whether local security policy blocks execution

The useful policy is bounded support plus verification, not "treat it as a
fully static Unix binary."

### 5. Declaring Installer Success Before The Tools Start

If the installer only downloads, extracts, and edits PATH, it can report
"success" on a machine where the user still cannot launch `kernc`.

That is not a valid official installation result.

## Failure Modes And First Checks

### The Tool Does Not Start On A Linux User Machine

First ask:

- was this installed from the official release archive
- or was it built locally on some different machine

Then check:

- whether `ldd` reports missing shared libraries
- whether the distro baseline is older than the machine used to produce the
  archive

If the runtime libraries are missing, install them.

If the machine is simply older than the release baseline, the practical answer
is to build Kern from source on the target machine.

### The Tool Does Not Start On A macOS User Machine

First ask:

- was this installed from the official release archive
- is the host within the supported macOS baseline
- did local security policy block execution

The first follow-up is not "make it static"; it is "inspect the actual macOS
loader/security failure and verify the shipped archive matches the claimed
target."

### The Package Script Says Success But The Archive Is Wrong

Verify:

- the script was run from the repository root
- the archive target matches the current host machine
- the binaries came from the real host-native release build
- the installer still verifies the installed tools after extraction

## Practical Summary

The practical rules are:

- Kern program runtime semantics and Unix host-tool distribution policy are
  separate concerns.
- Official Unix archives are currently host-native, not generic cross-target
  packaging artifacts.
- Official Unix archives must not promise full static portability they do not
  actually provide.
- Official Unix installers must verify that `kernc`, `craft`, and `kern-lsp`
  start before claiming success.
- Linux/macOS support should be described in terms of bounded host baselines,
  not vague "runs everywhere" language.
