# macOS SDK Slimming TODO

## Context

Linux and Windows default SDK packaging have now been tightened so an end-user
install no longer bundles a full LLVM development prefix just to run installed
tools.

The same cleanup should be completed for macOS.

The key rule remains:

- end-user SDK: smallest runtime-complete package that lets installed `kernc`,
  `craft`, and `kern-lsp` run correctly
- source-build / repo-dev environment: separate concern; users cloning the repo
  can configure host LLVM/Homebrew/Xcode tools themselves and should not force
  that payload into `~/.kern`
- standalone toolchain archive: the artifact that preserves a full
  source-build-oriented LLVM development prefix

Validate this work on real macOS hosts, not through emulation or cross-packaging.
Both official macOS host labels matter:

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

## Status From Previous Platforms

### Linux

Linux default SDK packaging already uses a runtime-complete subset instead of a
full LLVM development prefix.

### Windows

Windows was completed in commit:

```text
a65e6d2 perf(sdk): slim windows end-user toolchain
```

Measured Windows result:

- baseline SDK zip: `1460.08 MiB`
- minimized SDK zip: `125.54 MiB`
- reduction: `1334.54 MiB`
- reduction percent: `91.4%`

The measured Windows runtime tool set is:

- `clang.exe`
- `lld-link.exe`
- `llvm-lib.exe`

Windows explicitly does not bundle these in the default SDK:

- `clang++.exe`
- `llvm-ar.exe`
- `llvm-config.exe`
- full LLVM `include/`
- full LLVM `lib/`
- Clang resource dir

The standalone Windows `package-toolchain` artifact still preserves the full
development prefix.

## Main Goal

Shrink the default macOS SDK significantly without breaking:

- `install.sh`
- `python -m ops install`
- `kernc --version`
- `craft --version`
- `kern-lsp --version`
- direct `kernc hello_world.rn`
- `craft build`
- `craft run`
- macOS runtime entry modes currently covered by tests
- ThinLTO/LTO paths that rely on `ld64.lld` when configured

## Current Model To Inspect

The files most likely involved are:

- [ops/release.py](./ops/release.py)
- [ops/common.py](./ops/common.py)
- [ops/install.py](./ops/install.py)
- [install.sh](./install.sh)
- [compiler/kernc_driver/src/compiler/link.rs](./compiler/kernc_driver/src/compiler/link.rs)
- [manifest/ci-toolchains.json](./manifest/ci-toolchains.json)
- [docs/unix-distribution.md](./docs/unix-distribution.md)
- [docs/sdk-rebuild-plan.md](./docs/sdk-rebuild-plan.md)
- [README.md](./README.md)

The current code already has runtime-subset SDK packaging for Linux and Windows.
macOS still needs a real measurement pass before changing behavior because Mach-O
load commands, Homebrew paths, `install_name_tool`, and codesigning make the risk
different from Linux.

## Working Assumption

For the current end-user SDK, macOS likely needs only a runtime subset around:

- `clang`
- `ld64.lld`
- dynamic libraries required by those tools at runtime
- rewritten Mach-O load commands so bundled tools and bundled dylibs are
  relocatable inside the installed SDK
- possibly codesigning after load-command rewrites

Likely not required for the default end-user SDK unless proven otherwise:

- `clang++`
- `llvm-ar`
- `llvm-config`
- full LLVM `include/`
- full LLVM `lib/`
- full Clang resource dir
- extra LLVM utilities unrelated to Kern's current installed-user path

Do not assume these are removable without measurement. Prove the runtime set on
the macOS host where packaging is performed.

## Step 1: Pull And Establish Baseline

On the macOS machine:

```bash
git pull --ff-only
python3 -m ops release package --version v0.7.0-macos-baseline
ls -lh kern-v0.7.0-macos-baseline-*.tar.gz
```

Unpack and measure:

```bash
rm -rf tmp/macos-baseline
mkdir -p tmp/macos-baseline
tar -xzf kern-v0.7.0-macos-baseline-*.tar.gz -C tmp/macos-baseline
du -sh tmp/macos-baseline/kern-v0.7.0-macos-baseline-*/bin
du -sh tmp/macos-baseline/kern-v0.7.0-macos-baseline-*/lib/kern
du -sh tmp/macos-baseline/kern-v0.7.0-macos-baseline-*/toolchain/host/bin
du -sh tmp/macos-baseline/kern-v0.7.0-macos-baseline-*/toolchain/host/lib
du -sh tmp/macos-baseline/kern-v0.7.0-macos-baseline-*/toolchain/host/include
du -sh tmp/macos-baseline/kern-v0.7.0-macos-baseline-*/toolchain/host/sysroot
du -sh tmp/macos-baseline/kern-v0.7.0-macos-baseline-*/toolchain/host/lib/clang 2>/dev/null || true
```

Record exact numbers before changing code.

## Step 2: Identify macOS Runtime Tool Usage

Start from code:

- [compiler/kernc_driver/src/compiler/link.rs](./compiler/kernc_driver/src/compiler/link.rs)
- [ops/common.py](./ops/common.py)
- [ops/release.py](./ops/release.py)
- [manifest/ci-toolchains.json](./manifest/ci-toolchains.json)

Questions to answer:

1. Is `clang` still required as the driver for the installed-user link path?
2. When does `ld64.lld` get used?
3. Is `llvm-ar` needed for installed-user workflows, or only for tests/source
   builds/future `cc` work?
4. Is `clang++` needed at runtime at all?
5. Is `llvm-config` needed at runtime, or only for source builds and packaging?
6. Is the Clang resource dir touched by current macOS `kernc` link flows?
7. Do current macOS flows rely on Apple system `ld`, Xcode CLT tools, or SDK
   paths outside the bundled LLVM subset?

This task is about current installed-user flows, not future `kernc cc` support.

## Step 3: Inspect Mach-O Runtime Dependencies

For each candidate tool:

- `clang`
- `ld64.lld`
- maybe `llvm-ar` if relocatable/static archive paths prove it is needed
- maybe `clang++` only if a current installed-user path proves it is needed

inspect load commands:

```bash
otool -L "$(llvm-config --bindir)/clang"
otool -L "$(llvm-config --bindir)/ld64.lld" 2>/dev/null || true
```

If `ld64.lld` is provided outside the main LLVM prefix, locate it through the
existing Homebrew fallback logic and inspect that binary too.

Classify each dependency:

- macOS system library: do not bundle
- dependency under the selected LLVM/Homebrew prefix: bundle only if needed
- dependency under another Homebrew prefix such as `libxml2`, `zstd`, or
  similar: bundle only if the tool will not run without it

Important: copying a dylib is not enough. If a copied tool records absolute
Homebrew load-command paths, the packaged SDK must rewrite those paths to
`@loader_path`-relative references and codesign modified Mach-O files when
needed.

## Step 4: Build A Candidate Minimal macOS Toolchain

Create a scratch toolchain root:

```bash
MIN_TOOLCHAIN_ROOT="$(pwd)/tmp/macos-min-toolchain"
rm -rf "$MIN_TOOLCHAIN_ROOT"
mkdir -p "$MIN_TOOLCHAIN_ROOT/bin" "$MIN_TOOLCHAIN_ROOT/lib"

cp "$(llvm-config --bindir)/clang" "$MIN_TOOLCHAIN_ROOT/bin/"
# Adjust this if ld64.lld comes from a separate Homebrew formula.
cp "$(dirname "$(command -v ld64.lld)")/ld64.lld" "$MIN_TOOLCHAIN_ROOT/bin/"
```

Add only required dylibs discovered from `otool -L`.

Rewrite load commands if needed:

```bash
otool -L "$MIN_TOOLCHAIN_ROOT/bin/clang"
otool -L "$MIN_TOOLCHAIN_ROOT/bin/ld64.lld"
```

Use `install_name_tool -change` for copied non-system dylibs that still point at
absolute paths. Re-sign modified files if macOS refuses to execute them:

```bash
codesign --force --sign - "$MIN_TOOLCHAIN_ROOT/bin/clang"
codesign --force --sign - "$MIN_TOOLCHAIN_ROOT/bin/ld64.lld"
```

Then check startup from the isolated directory:

```bash
"$MIN_TOOLCHAIN_ROOT/bin/clang" --version
"$MIN_TOOLCHAIN_ROOT/bin/ld64.lld" --version
```

Do not copy the full Homebrew or LLVM `bin/` directory just to make startup pass.

## Step 5: Validate Minimal Toolchain Against Kern

Prepare a minimal source:

```bash
mkdir -p tmp
cat > tmp/hello_world.rn <<'EOF'
fn main() i32 {
    return 0;
}
EOF
```

Validate direct compile and run:

```bash
cargo run -q -p kernc_cli -- --toolchain-root "$MIN_TOOLCHAIN_ROOT" tmp/hello_world.rn -o tmp/hello_world
./tmp/hello_world
```

Also validate:

- direct source build defaults
- `--runtime-entry rt`
- `-c`
- `--link-only` only with a complete set of required runtime/link inputs
- one `craft build`
- one `craft run`

For `craft`, prefer environment injection because current `craft` does not expose
a direct `--toolchain-root` flag:

```bash
export KERN_TOOLCHAIN_ROOT="$MIN_TOOLCHAIN_ROOT"
rm -rf tmp/hello
mkdir -p tmp/hello
(
  cd tmp/hello
  cargo run -q -p craft -- init
  cargo run -q -p craft -- build
  cargo run -q -p craft -- run
)
```

If something breaks, identify whether the missing part is:

- another executable
- a dylib
- a resource directory
- a system SDK / Xcode CLT dependency
- a driver assumption in `link.rs`
- a Mach-O load-command rewrite problem
- a codesigning problem

## Step 6: Change Packaging Logic

Once the runtime-complete set is proven, update packaging so the default macOS
SDK follows the same runtime-only principle as Linux and Windows.

Likely files to edit:

- [ops/release.py](./ops/release.py)
- [ops/common.py](./ops/common.py)
- [ops/install.py](./ops/install.py)
- [install.sh](./install.sh)

Expected changes:

1. Route `*-apple-darwin` default SDK packaging through a macOS runtime-subset
   bundling path.
2. Stop copying full LLVM `bindir/libdir/includedir` into the default macOS SDK.
3. Copy only:
   - required runtime executables
   - required runtime dylibs
   - only proven-required resource directories
4. Rewrite Mach-O load commands for copied non-system dylibs.
5. Codesign modified Mach-O files when required.
6. Keep standalone `package-toolchain` as the full development-prefix artifact.
7. Ensure `sdk.json` describes the runtime-only toolchain contract accurately.

Important:

- do not break Linux and Windows runtime-subset packaging
- do not break standalone `package-toolchain`
- do not break CI assumptions that intentionally validate full toolchain archives

## Step 7: Adjust Installer Validation

Review:

- [install.sh](./install.sh)
- [ops/install.py](./ops/install.py)

Installer validation should verify:

- SDK manifest exists
- host target matches
- `kernc`, `craft`, `kern-lsp` exist
- required bundled runtime toolchain components exist
- required bundled runtime toolchain components actually start
- installed `kernc`, `craft`, and `kern-lsp` start

Installer validation should not require:

- full LLVM `include/`
- full LLVM `lib/`
- `llvm-config`
- full Clang resource dir
- a source-build-ready LLVM development prefix

For macOS, startup validation must catch broken dylib load commands. A tool that
exists but cannot start is a packaging failure.

## Step 8: Re-run Packaging And Measure Improvement

After code changes:

```bash
python3 -m ops release package --version v0.7.0-macos-minsdk
ls -lh kern-v0.7.0-macos-minsdk-*.tar.gz
```

Unpack and compare:

```bash
rm -rf tmp/macos-minsdk
mkdir -p tmp/macos-minsdk
tar -xzf kern-v0.7.0-macos-minsdk-*.tar.gz -C tmp/macos-minsdk
du -sh tmp/macos-minsdk/kern-v0.7.0-macos-minsdk-*/bin
du -sh tmp/macos-minsdk/kern-v0.7.0-macos-minsdk-*/lib/kern
du -sh tmp/macos-minsdk/kern-v0.7.0-macos-minsdk-*/toolchain/host/bin
du -sh tmp/macos-minsdk/kern-v0.7.0-macos-minsdk-*/toolchain/host/lib
du -sh tmp/macos-minsdk/kern-v0.7.0-macos-minsdk-*/toolchain/host/include 2>/dev/null || true
du -sh tmp/macos-minsdk/kern-v0.7.0-macos-minsdk-*/toolchain/host/lib/clang 2>/dev/null || true
```

Report:

- baseline archive size
- new archive size
- absolute reduction
- percentage reduction
- runtime tools included
- runtime dylibs included
- whether Clang resource dir is included or omitted, with proof

## Step 9: Full End-User Validation On Real macOS

Do not stop at packaging.

Validate the user path end-to-end from a local archive:

```bash
rm -rf "$HOME/.kern"
./install.sh --archive ./kern-v0.7.0-macos-minsdk-*.tar.gz
"$HOME/.kern/bin/kernc" --version
"$HOME/.kern/bin/craft" --version
"$HOME/.kern/bin/kern-lsp" --version
```

Then compile:

```bash
"$HOME/.kern/bin/kernc" tmp/hello_world.rn -o tmp/hello_world
./tmp/hello_world
```

Then test `craft`:

```bash
rm -rf tmp/hello-installed
mkdir -p tmp/hello-installed
(
  cd tmp/hello-installed
  "$HOME/.kern/bin/craft" init
  "$HOME/.kern/bin/craft" build
  "$HOME/.kern/bin/craft" run
)
```

Also validate `python3 -m ops install` from the same local archive:

```bash
python3 -m ops install --archive ./kern-v0.7.0-macos-minsdk-*.tar.gz --dest ./tmp/kern-install-macos --no-path
./tmp/kern-install-macos/bin/kernc --version
./tmp/kern-install-macos/bin/craft --version
./tmp/kern-install-macos/bin/kern-lsp --version
```

## Step 10: Standalone Toolchain Artifact Validation

Confirm `package-toolchain` still preserves the full development prefix:

```bash
python3 -m ops release package-toolchain --version llvm-21.1.8-macos-fullcheck
rm -rf tmp/macos-toolchain-fullcheck
mkdir -p tmp/macos-toolchain-fullcheck
tar -xzf kern-toolchain-llvm-21.1.8-macos-fullcheck-*.tar.gz -C tmp/macos-toolchain-fullcheck
du -sh tmp/macos-toolchain-fullcheck/kern-toolchain-llvm-21.1.8-macos-fullcheck-*/toolchain/host/bin
du -sh tmp/macos-toolchain-fullcheck/kern-toolchain-llvm-21.1.8-macos-fullcheck-*/toolchain/host/lib
du -sh tmp/macos-toolchain-fullcheck/kern-toolchain-llvm-21.1.8-macos-fullcheck-*/toolchain/host/include
```

Inspect `manifest/toolchain.json` and confirm it still records:

- `include_dir`
- `llvm_config`
- `clang_resource_dir` when available
- full development-prefix layout

## Step 11: Regression Tests To Run

At minimum, rerun:

```bash
cargo test -q -p kernc_driver
cargo test -q -p kernc_cli --test stdlib
cargo test -q -p craft
```

If time permits, run:

```bash
cargo test --workspace
```

If macOS-specific linking behavior changes, add or adjust targeted tests rather
than relying only on packaging smoke tests.

## Step 12: Documentation Updates

Once macOS is confirmed, update:

- [README.md](./README.md)
- [docs/unix-distribution.md](./docs/unix-distribution.md)
- [docs/sdk-rebuild-plan.md](./docs/sdk-rebuild-plan.md)

Documentation should clearly state:

- default SDK is runtime-complete, not a full LLVM development prefix
- Linux, Windows, and macOS now follow the same user-SDK principle
- full dev-oriented toolchain remains available separately
- source builds from Git clone are expected to configure host LLVM/Xcode/Homebrew
  environment directly

## Acceptance Criteria

This task is done only if all of the following are true:

1. The default macOS SDK archive is materially smaller than before.
2. `install.sh` works from a local archive.
3. `python3 -m ops install` works from a local archive.
4. Installed `kernc`, `craft`, and `kern-lsp` all start successfully.
5. Installed `kernc hello_world.rn -o hello_world` succeeds.
6. The resulting `hello_world` binary runs.
7. Installed `craft build` and `craft run` succeed.
8. The standalone toolchain artifact still preserves the full development prefix.
9. Manifest and installer semantics no longer assume the user SDK is a
   source-build environment.
10. The final implementation does not regress Linux or Windows SDK packaging.

## Nice-To-Have Follow-Ups

These are not required to finish macOS SDK slimming, but note them if discovered:

- add a packaging-time smoke test that validates each minimal SDK archive before
  release
- add an explicit manifest distinction between `runtime-toolchain` and
  `dev-toolchain`
- record per-platform runtime tool sets in docs or manifest policy
- later, if `kernc cc` lands, reevaluate which Clang resources become
  runtime-required
- consider whether standalone toolchain artifacts can also be trimmed in a
  separate task without losing source-build utility

## Final Reminder

Do not optimize for matching the old SDK layout.

Optimize for:

- smallest user download that still works
- clear separation between installed-user runtime and repo-development
  environment
- explicit proof for every bundled macOS component
- preserving the standalone development toolchain artifact
