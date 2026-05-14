# Installing Kern

This document is the central guide for installing, verifying, and reproducing a
Kern toolchain installation.

Platform-specific release constraints live in
[Unix Distribution](./unix-distribution.md) and
[Windows Distribution](./windows-distribution.md). Those documents explain why
the archives are built and labeled the way they are; this document explains how
users and maintainers install and inspect them.

## Installed SDK Layout

The official SDK installs into:

```text
~/.kern
```

on Unix, and:

```text
%USERPROFILE%\.kern
```

on Windows.

The installed tree contains:

- `bin/kernc`: compiler, analysis, object emission, and linking driver
- `bin/craft`: package manager and build orchestrator
- `bin/kern-lsp`: language server for editor integration
- `lib/kern`: official library workspace, including `base`, `rt`, and `std`
- `toolchain/host`: bundled host LLVM/Clang runtime tools required by the SDK
- `manifest/sdk.json`: SDK identity, host target, toolchain component records,
  checksums, sizes, and health-check expectations

Installed commands resolve the SDK-owned library roots and bundled host tools
relative to the active SDK layout. Users normally only need `bin` on PATH; they
do not need to set `KERNLIB_PATH`, `KERN_TOOLCHAIN_ROOT`, or LLVM environment
variables for ordinary installed-SDK use.

The default SDK is an end-user SDK. It intentionally does not carry the full
LLVM development prefix needed to build Kern from source. Full development
toolchain archives are produced separately with `package-toolchain`.

## Recommended Install

Linux and macOS:

```sh
curl -sSf https://raw.githubusercontent.com/kern-project/kern/main/install.sh | bash
```

Windows PowerShell:

```powershell
powershell -Command "Set-ExecutionPolicy Bypass -Scope Process -Force; Invoke-Expression (Invoke-WebRequest -Uri https://raw.githubusercontent.com/kern-project/kern/main/install.ps1 -UseBasicParsing).Content"
```

The installer downloads the host-native release archive, installs it into the
default SDK root, configures PATH, and verifies that `kernc`, `craft`, and
`kern-lsp` start successfully.

## Installer Options

Unix:

```sh
sh ./install.sh --help
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -?
```

The common options are:

- `--version <tag>` / `-Version <tag>`: install a specific release tag
- `--target <target>` / `-Target <target>`: select the host archive label
- `--archive <path>` / `-Archive <path>`: install from a local SDK archive
- `--dest <path>` / `-Dest <path>`: install into a custom directory
- `--no-path` / `-NoPath`: skip PATH mutation
- `--github-repo <repo>` / `-GitHubRepo <repo>`: override the release source

The target option is not a cross-install knob. It must match the current host.

## Offline Installs

Download the SDK archive once, then pass it to the installer.

Unix:

```sh
sh ./install.sh --archive ./kern-v0.7.6-x86_64-linux-gnu.tar.gz
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Archive .\kern-v0.7.6-x86_64-windows-msvc.zip
```

If the archive filename does not contain the release tag, pass it explicitly.

Unix:

```sh
sh ./install.sh --version v0.7.6 --archive ./kern.tar.gz
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Version v0.7.6 -Archive .\kern.zip
```

## Rust Installer Entry Point

`kernup` is the Rust SDK installer and future toolchain manager entry point. It
is useful when working from a source checkout.

Install a release archive directly:

```sh
cargo run -p kernup -- install --version v0.7.6
```

Install a local archive:

```sh
cargo run -p kernup -- install --archive ./kern-v0.7.6-<host-target>.<tar.gz|zip>
```

Print the current host archive target:

```sh
cargo run -p kernup -- target
```

Validate the default installation:

```sh
cargo run -p kernup -- doctor
```

The repository-root shell and PowerShell installers remain the user-facing
contract. `kernup` is the repository engineering surface and will grow into the
toolchain manager over time.

## Building From Source

For local compiler development:

```sh
git clone https://github.com/kern-project/kern.git
cd kern
cargo build --release
cargo test
```

This produces local development binaries under:

```text
target/release/
```

That is not the same as an installed SDK. A source build may depend on local
development tools and environment variables, especially the full LLVM 21
development prefix required by `llvm-sys`.

Windows source builds require a complete LLVM 21 development prefix, Visual
Studio Build Tools for the MSVC target, and the LLVM-side `libxml2` dependency.
See [Windows Distribution](./windows-distribution.md#local-development-build)
for the exact setup.

## Creating A Local SDK Archive

If the installed SDK must remain usable after deleting the source checkout,
package an SDK archive and install that archive.

From the repository root:

```sh
cargo run -q -p kernworker -- release package --version v0.7.6 --target <host-target>
```

Examples:

```sh
cargo run -q -p kernworker -- release package --version v0.7.6 --target x86_64-linux-gnu
cargo run -q -p kernworker -- release package --version v0.7.6 --target x86_64-apple-darwin
cargo run -q -p kernworker -- release package --version v0.7.6 --target aarch64-apple-darwin
cargo run -q -p kernworker -- release package --version v0.7.6 --target x86_64-windows-msvc
```

Then install the archive with either the platform installer or `kernup`.

The packaging command is intentionally host-native for the current release
model. The archive label must match the current host and the binaries copied
into the archive.

## Full Toolchain Archives

The default SDK contains only the runtime LLVM/Clang tools needed by installed
Kern commands. A full LLVM development prefix is packaged separately:

```sh
cargo run -q -p kernworker -- release package-toolchain --version llvm-21.1.8 --target <host-target>
```

That archive writes `manifest/toolchain.json` and is intended for CI,
release-engineering, and source-build workflows. It is deliberately separate
from the smaller end-user SDK.

## Verification And Reproducibility

Every successful install should prove at least:

```sh
kernc --version
craft --version
kern-lsp --version
```

For deeper inspection, read:

```text
~/.kern/manifest/sdk.json
```

or the equivalent manifest under a custom install root. The manifest records the
host target, SDK version, bundled toolchain provenance, component paths,
checksums, sizes, and health checks.

The most important reproducibility rules are:

- install from the official release archive or record the exact local archive
  that was installed
- keep the archive target label equal to the real host target
- use `kernworker release package` for SDK archives, not ad hoc copies from
  `target/release`
- verify installed tools after extraction, before declaring installation
  successful
- use source builds on hosts older or more constrained than the current release
  baseline

## Platform Notes

Current official release baselines and footguns are documented separately:

- [Unix Distribution](./unix-distribution.md)
- [Windows Distribution](./windows-distribution.md)

Those documents are intentionally about host-tool distribution policy, not Kern
program runtime semantics.
