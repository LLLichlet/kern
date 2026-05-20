# Versioning Policy

This document defines Kern release numbering, Craft compatibility declarations,
and the release-train rules used by repository maintainers.

Kern is still pre-1.0. Language, standard library, and package-manager behavior
may still change incompatibly, but release numbers should be predictable enough
for users and ecosystem projects to plan upgrades.

## Version Format

Kern releases use standard SemVer-shaped versions:

```text
0.MINOR.PATCH
```

Release tags add the conventional `v` prefix:

```text
v0.MINOR.PATCH
```

Do not add a fourth numeric component. Do not roll patch numbers over at
arbitrary thresholds such as 20 or 50. A version like `0.7.37` is acceptable if
the `0.7` line needs that many maintenance releases.

## Meaning Before 1.0

Before `1.0`, Kern treats the minor number as the compatibility and migration
line.

- `PATCH` releases are for packaging fixes, installer fixes, editor fixes,
  documentation corrections, CI/release fixes, and low-risk compiler or tool
  fixes that should not require ecosystem migration.
- `MINOR` releases are for language changes, standard library layout changes,
  Craft manifest or lockfile behavior changes, and other changes that ecosystem
  projects should consciously migrate to.
- `MAJOR` remains `0` until the project is ready to make a stronger stability
  commitment.

Patch releases may still contain bug fixes that change incorrect behavior, but
they should not intentionally change the migration line.

## Pre-Releases

Avoid alpha and beta releases unless there is a concrete external testing need.
They create more tags, archives, registry versions, and support states.

When a planned minor line needs release-candidate validation, use ordinary
SemVer pre-release labels:

```text
0.8.0-rc.1
0.8.0-rc.2
```

Do not publish pre-release crates or Marketplace extensions unless the release
candidate is meant to be publicly installable and supportable. Prefer CI
artifacts for internal validation.

## Craft Compatibility

Craft manifests declare the Kern compatibility line with `[package].kern`.

For ordinary packages, prefer the minor line:

```toml
[package]
name = "demo"
version = "0.1.0"
kern = "0.8"
```

That declaration accepts any `0.8.PATCH` toolchain. It is the default generated
by `craft init` and should be used by ecosystem packages unless they knowingly
depend on a patch-specific behavior.

A full patch version is still accepted:

```toml
kern = "0.8.0"
```

Use a full patch version only for packages that require that exact toolchain
patch level. A manifest from another minor line, such as `kern = "0.9"`, must
not be accepted by a `0.8.PATCH` toolchain.

## Source Of Truth

The repository root `Cargo.toml` `[workspace.package].version` is the canonical
checked-in toolchain version. Other checked-in references, such as install
examples, `kernup` examples, the VS Code extension package, Nix metadata, and
tests, are synchronized from it for release branches.

Use `kernworker` for release version bumps:

```sh
cargo run -q -p kernworker -- release bump-version --version 0.7.10
```

Check that a branch is synchronized:

```sh
cargo run -q -p kernworker -- release bump-version --version 0.7.10 --check
```

The command rewrites tracked UTF-8 files only. Ignored VSIX archives,
`node_modules`, build output, and binary artifacts are not touched.

## Release Order

Use this order for public releases:

1. Bump the repository version with `kernworker release bump-version`.
2. Build platform SDK and full-toolchain artifacts in CI.
3. Smoke install the generated SDK on native runners and configured Linux
   distro probes.
4. Publish GitHub release assets only after SDK smoke checks pass.
5. Publish crates.io packages after SDK release validation.
6. Publish the VS Code Marketplace extension after the VSIX package has been
   validated.

If a tag, release asset, crates.io version, or Marketplace version has already
been published and turns out to be bad, do not delete and reuse that version.
Publish the next patch version instead.

## Ecosystem Upgrade Policy

Ecosystem repositories should usually update `[package].kern` only when moving
to a new minor line, for example from `0.7` to `0.8`.

Patch releases should not require mass ecosystem manifest churn. If a patch
release does require ecosystem changes, treat that as a release-process smell and
consider whether the change should have waited for the next minor line.
