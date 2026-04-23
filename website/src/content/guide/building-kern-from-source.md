---
title: "Building Kern From Source"
summary: "Build `kernc`, `craft`, and `kern-lsp` directly from the repository when you need a local host-tool build."
order: 4
---

This chapter is for the case where you want to build the host tools themselves
from this repository:

- `kernc`
- `craft`
- `kern-lsp`

That is different from using an already-installed toolchain to build Kern
programs.

## When To Use A Source Build

A source build is the right path when:

- you are developing Kern itself
- you want to test the current repository state rather than a packaged release
- the official release archive does not match your machine well enough
- the installer tells you to build on the target machine instead

If you just want to write and run Kern code, the packaged installer is still
the simpler path.

## What You Need First

Before building from source, make sure the machine already has:

- the latest stable Rust toolchain
- a working C++ compiler
- LLVM development libraries and headers
- `llvm-config` available for the same LLVM version that `llvm-sys` resolves

On Unix-like systems, the critical boundary is simple: `cargo build --release`
must be able to compile Rust code and the small C++ ThinLTO bridge in
`compiler/kernc_codegen/src/thinlto_bridge.cpp`.

## Clone And Build

Clone the repository and build the release workspace:

```bash
git clone https://github.com/softfault/kern.git
cd kern
cargo build --release
```

That produces the host tools under `target/release/`:

- `target/release/kernc`
- `target/release/craft`
- `target/release/kern-lsp`

If you only want one tool, build that package explicitly:

```bash
cargo build --release -p kernc_cli --bin kernc
cargo build --release -p craft
cargo build --release -p kern-lsp
```

## Verify The Result

After the build finishes, run the tools directly from `target/release/`:

```bash
target/release/kernc --version
target/release/craft --version
target/release/kern-lsp --help
```

If you want to use those binaries repeatedly during local development, either:

- call them by full path from `target/release/`
- or add `target/release/` to your shell `PATH`

That is the right path while the repository checkout stays on disk.

If you want to build from a clone, install the resulting toolchain into the
normal Kern home, and then delete the clone directory, package a local SDK
archive first instead of relying on `target/release/` directly.

## Install A Local SDK From The Clone

For a local self-built install that survives deleting the checkout, use the
repository packaging and install entrypoints.

On Linux or macOS:

```bash
cargo build --release
python3 -m ops release package --version v0.7.1-local --target <host-target>
python3 -m ops install --archive ./kern-v0.7.1-local-<host-target>.tar.gz --no-path
```

or:

```bash
./install.sh --archive ./kern-v0.7.1-local-<host-target>.tar.gz --no-path
```

On Windows:

```powershell
cargo build --release
py -3 -m ops release package --version v0.7.1-local --target x86_64-windows-msvc
py -3 -m ops install --archive .\kern-v0.7.1-local-x86_64-windows-msvc.zip --no-path
```

or:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Archive .\kern-v0.7.1-local-x86_64-windows-msvc.zip
```

That produces the same installed SDK layout the normal release installer uses:

- `~/.kern` on Unix
- `%USERPROFILE%\.kern` on Windows

## Windows Note

For local development on Windows, plain Cargo release builds are fine:

```powershell
cargo build --release
```

But that is not the official release-packaging path. If you want official-style
Windows host tools with static CRT enabled, use:

```powershell
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release --target x86_64-pc-windows-msvc -p kernc_cli --bin kernc
cargo build --release --target x86_64-pc-windows-msvc -p craft
cargo build --release --target x86_64-pc-windows-msvc -p kern-lsp
```

Or use the repository packaging entrypoint:

```powershell
py -3 -m ops release package --version v0.7.1 --target x86_64-windows-msvc
```

## Troubleshooting

If the source build fails, the most common causes are:

- Rust is installed, but LLVM development headers/libs are missing
- `llvm-config` points at a different LLVM version than the one Cargo resolves
- the C++ compiler is present, but the system C/C++ development headers are incomplete
- an older checkout injected `-isystem /usr/include` while compiling
  `thinlto_bridge.cpp`, which can break GCC/libstdc++ header resolution on
  Linux distributions such as Arch

If the error comes from `thinlto_bridge.cpp`, check the host C++ and LLVM setup
first. That file is the small non-Rust bridge that links Kern's ThinLTO support
to LLVM.

If you specifically see `<cstdlib>` failing to find `stdlib.h` while the compile
command includes `-isystem /usr/include`, update to a newer checkout. That was
an actual Kern build-script bug rather than a normal package-install problem.

For deeper host-tool distribution and packaging policy, see:

- `README.md`
- `docs/kernc.md`
- `docs/unix-distribution.md`
- `docs/windows-distribution.md`
