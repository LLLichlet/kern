# Windows SDK Slimming TODO

## Context

Linux side has already been tightened so the end-user SDK no longer bundles a
full LLVM development prefix just to run installed tools.

The same cleanup now needs to be done on Windows.

The key rule is:

- end-user SDK: smallest runtime-complete package that lets installed `kernc`,
  `craft`, and `kern-lsp` run correctly
- source-build / repo-dev environment: separate concern; users cloning the repo
  can configure host LLVM/MSVC themselves and should not force that payload into
  `%USERPROFILE%\\.kern`

This work should be validated on a real Windows machine, not by `wine`.

## Main Goal

Shrink the default Windows SDK significantly without breaking:

- `install.ps1`
- `kernc --version`
- `craft --version`
- `kern-lsp --version`
- direct `kernc hello_world.rn`
- `craft build`
- `craft run`
- Windows runtime entry / CRT modes currently covered by tests

## Current Model To Revisit

Today the SDK packaging path still conflates two use cases:

- user install of a ready-to-run toolchain
- source-build-friendly LLVM development prefix

Current packaging behavior to inspect:

- [ops/release.py](./ops/release.py)
- [ops/common.py](./ops/common.py)
- [ops/install.py](./ops/install.py)
- [compiler/kernc_driver/src/compiler/link.rs](./compiler/kernc_driver/src/compiler/link.rs)
- [install.ps1](./install.ps1)
- [docs/windows-distribution.md](./docs/windows-distribution.md)

On Linux, the large waste came from bundling:

- full LLVM `bin/`
- full LLVM `lib/`
- full LLVM `include/`
- full `clang resource dir`

Windows likely has the same structural problem, plus DLL/runtime details.

## Working Assumption

For the current end-user SDK, Windows likely needs only a runtime subset around:

- `clang.exe`
- `lld-link.exe`
- `llvm-lib.exe`
- any DLLs those tools actually require at runtime
- any files those tools require for Kern's current use as a link driver

Likely **not** required for the default end-user SDK unless proven otherwise:

- `clang++.exe`
- `llvm-ar.exe`
- `llvm-config.exe`
- full LLVM `include/`
- full LLVM `lib/`
- full `clang resource dir`
- extra LLVM utilities unrelated to Kern's current installed-user path

Important: do not assume these are removable without measurement. Prove it.

## Required Outcome

At the end of this task, there should be a clear Windows packaging split:

- default SDK archive = end-user runtime-complete package
- standalone toolchain archive / repo environment = full source-build-oriented toolchain

## Step 1: Measure The Current Windows Baseline

On the Windows machine:

1. Pull latest changes.
2. Build or reuse current release binaries.
3. Package a baseline SDK archive.
4. Record exact sizes before changing anything.

Suggested commands:

```powershell
git pull
py -3 -m ops release package --version v0.7.0-win-baseline
Get-Item .\kern-v0.7.0-win-baseline-x86_64-windows-msvc.zip | Select-Object Name,Length
```

Also unpack it and measure the top-level contributors:

```powershell
Expand-Archive .\kern-v0.7.0-win-baseline-x86_64-windows-msvc.zip -DestinationPath .\tmp\win-baseline -Force
Get-ChildItem .\tmp\win-baseline\kern-v0.7.0-win-baseline-x86_64-windows-msvc -Force
```

Then specifically measure:

- `bin/`
- `lib/kern/`
- `toolchain/host/bin`
- `toolchain/host/lib`
- `toolchain/host/include`
- `toolchain/host/sysroot`

Write the numbers down in a scratch note before changing code.

## Step 2: Identify The Real Runtime Tool Set

Determine which Windows LLVM tools are actually used by installed Kern tools.

Start from code:

- [compiler/kernc_driver/src/compiler/link.rs](./compiler/kernc_driver/src/compiler/link.rs)
- [ops/common.py](./ops/common.py)
- [manifest/ci-toolchains.json](./manifest/ci-toolchains.json)

Questions to answer:

1. Is `clang.exe` still required as the driver for the link path?
2. When does `lld-link.exe` get used?
3. When does `llvm-lib.exe` get used?
4. Is `clang++.exe` needed for the installed-user path at all?
5. Is `llvm-ar.exe` needed for installed-user workflows, or only for tests / future `cc` work?
6. Is `llvm-config.exe` needed at runtime, or only for source builds and packaging?
7. Is `clang resource dir` actually touched by current Windows `kernc` link flows?

You are not trying to predict future `kernc cc` yet. This task is about the
current installed-user path.

## Step 3: Inspect Windows Runtime Dependencies

The Linux cleanup succeeded because the runtime set was tiny. On Windows, the
main risk is hidden DLL dependencies.

For each candidate tool:

- `clang.exe`
- `lld-link.exe`
- `llvm-lib.exe`
- maybe `clang++.exe` if still under consideration

find what runtime files they actually need.

Do this empirically:

1. Copy only the candidate `.exe` files into a temp directory.
2. Try running `--version`.
3. Add DLLs until they start reliably.
4. Record the minimal passing set.

Example scratch workflow:

```powershell
$tmp = Join-Path $env:TEMP "kern-win-min-toolchain"
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path "$tmp\bin" | Out-Null
Copy-Item "C:\Path\To\LLVM\bin\clang.exe" "$tmp\bin\"
Copy-Item "C:\Path\To\LLVM\bin\lld-link.exe" "$tmp\bin\"
Copy-Item "C:\Path\To\LLVM\bin\llvm-lib.exe" "$tmp\bin\"
& "$tmp\bin\clang.exe" --version
```

If startup fails, inspect what DLLs are missing and add only those.

Do not assume the whole LLVM `bin/` directory is needed just because some DLLs
live there.

## Step 4: Validate Minimal Toolchain Against Kern

After finding a candidate minimal Windows runtime toolchain directory, validate
it against actual Kern flows before changing packaging.

Use `--toolchain-root` to force Kern onto the candidate directory.

Prepare a minimal test source:

```powershell
@'
fn main() i32 {
    return 0;
}
'@ | Set-Content .\tmp\hello_world.rn
```

Validation commands:

```powershell
cargo run -q -p kernc_cli -- --toolchain-root <MIN_TOOLCHAIN_ROOT> .\tmp\hello_world.rn -o .\tmp\hello_world.exe
.\tmp\hello_world.exe
```

Then also validate:

- direct source build defaults
- `--runtime-entry rt`
- `-c`
- `--link-only` if relevant
- one `craft build`
- one `craft run`

If something breaks, identify whether the missing part is:

- another `.exe`
- a DLL
- a library file
- a resource dir
- a driver assumption in `link.rs`

## Step 5: Change Packaging Logic

Once the runtime-complete set is proven, update packaging so the default
Windows SDK follows the same principle as Linux.

Likely files to edit:

- [ops/release.py](./ops/release.py)
- [ops/common.py](./ops/common.py)
- [ops/install.py](./ops/install.py)

What to change:

1. Introduce a Windows runtime-subset bundling path for the SDK archive.
2. Stop copying full LLVM `bindir/libdir/includedir` into the default SDK.
3. Copy only:
   - required runtime executables
   - required runtime DLLs / support files
   - only proven-required resource directories
4. Keep the standalone `package-toolchain` artifact as the place that preserves
   the full development prefix.
5. Adjust `sdk.json` generation so the SDK manifest reflects the reduced
   runtime-only toolchain contract.

Important:

- do not break the standalone toolchain artifact
- do not break CI assumptions that intentionally validate the full toolchain archive

## Step 6: Adjust Installer Validation

The installer currently validates more than the end-user SDK should guarantee.

Review:

- [install.ps1](./install.ps1)
- [ops/install.py](./ops/install.py)

Update validation so the installed SDK checks only the runtime-complete subset.

The installer should verify:

- SDK manifest exists
- host target matches
- `kernc`, `craft`, `kern-lsp` exist
- required bundled runtime toolchain components exist
- those tools actually start

The installer should **not** require a full LLVM development prefix for a user
install.

## Step 7: Re-run Real Packaging And Measure Improvement

After code changes:

```powershell
py -3 -m ops release package --version v0.7.0-win-minsdk
Get-Item .\kern-v0.7.0-win-minsdk-x86_64-windows-msvc.zip | Select-Object Name,Length
```

Then compare directly against the baseline from Step 1.

Report:

- baseline zip size
- new zip size
- absolute reduction
- percentage reduction

Also inspect unpacked contents to ensure the shrink came from toolchain cleanup,
not from accidentally dropping required Kern assets.

## Step 8: Full End-User Validation On Real Windows

Do not stop at packaging.

Validate the user path end-to-end:

1. Remove any old install.
2. Install from local archive with `install.ps1 -Archive`.
3. Ensure the installed tools start.
4. Compile and run a minimal program.
5. Build and run a minimal `craft` package.

Suggested flow:

```powershell
Remove-Item $env:USERPROFILE\.kern -Recurse -Force -ErrorAction SilentlyContinue
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Archive .\kern-v0.7.0-win-minsdk-x86_64-windows-msvc.zip
$env:USERPROFILE\.kern\bin\kernc.exe --version
$env:USERPROFILE\.kern\bin\craft.exe --version
$env:USERPROFILE\.kern\bin\kern-lsp.exe --version
```

Then compile:

```powershell
$env:USERPROFILE\.kern\bin\kernc.exe .\tmp\hello_world.rn -o .\tmp\hello_world.exe
.\tmp\hello_world.exe
```

Then test `craft`:

```powershell
New-Item -ItemType Directory -Force -Path .\tmp\hello | Out-Null
Push-Location .\tmp\hello
& $env:USERPROFILE\.kern\bin\craft.exe init --name hello
& $env:USERPROFILE\.kern\bin\craft.exe build
& $env:USERPROFILE\.kern\bin\craft.exe run
Pop-Location
```

## Step 9: Regression Tests To Run

At minimum, rerun:

```powershell
cargo test -q -p kernc_driver
cargo test -q -p kernc_cli --test stdlib
cargo test -q -p craft
```

If time permits, run full workspace tests on Windows after the packaging change.

## Step 10: Update Documentation

Once Windows is confirmed:

- [README.md](./README.md)
- [docs/windows-distribution.md](./docs/windows-distribution.md)
- [docs/sdk-rebuild-plan.md](./docs/sdk-rebuild-plan.md)

Documentation should clearly state:

- default SDK is runtime-complete, not a full LLVM dev prefix
- full dev-oriented toolchain remains available separately
- source builds from Git clone are expected to configure the host environment
  directly

## Acceptance Criteria

This task is done only if all of the following are true:

1. The default Windows SDK archive is materially smaller than before.
2. `install.ps1` still works from a local archive.
3. Installed `kernc`, `craft`, and `kern-lsp` all start successfully.
4. `kernc hello_world.rn -o hello_world.exe` succeeds on the installed SDK.
5. `craft build` and `craft run` succeed on the installed SDK.
6. The standalone toolchain artifact still preserves the full development prefix.
7. Manifest and installer semantics no longer assume the user SDK is a source-build environment.

## Nice-To-Have Follow-Ups

These are not required to finish this task, but note them if discovered:

- determine whether Windows can also drop `clang resource dir`
- determine whether `clang++.exe` can be removed from the standalone SDK path
- add a packaging-time smoke test that validates the minimal SDK archive before release
- add a manifest distinction between `runtime-toolchain` and `dev-toolchain`
- later, if `kernc cc` lands, reevaluate which Clang resources become runtime-required

## Files Likely To Change

- [ops/release.py](./ops/release.py)
- [ops/common.py](./ops/common.py)
- [ops/install.py](./ops/install.py)
- [install.ps1](./install.ps1)
- [compiler/kernc_driver/src/compiler/link.rs](./compiler/kernc_driver/src/compiler/link.rs)
- [README.md](./README.md)
- [docs/windows-distribution.md](./docs/windows-distribution.md)
- [docs/sdk-rebuild-plan.md](./docs/sdk-rebuild-plan.md)

## Final Reminder

Do not optimize for "matching the old SDK layout".

Optimize for:

- smallest user download that still works
- clear separation between installed-user runtime and repo-development environment
- explicit proof for every bundled Windows component
