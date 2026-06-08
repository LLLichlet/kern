# Licensing Policy

Kern uses a split licensing model.

The core toolchain is copyleft. It is licensed under the GNU General Public
License version 3 or later:

- `compiler/`
- `tools/craft/`
- `tools/lsp/`
- `tools/kernworker/`

The user-facing libraries, installer, shared helper crates, examples, editor
extension, documentation, package templates, and other repository material are
licensed under the MIT License unless a file or package says otherwise:

- `library/`
- `tools/kernup/`
- `shared/`
- `examples/`
- `editors/`
- `docs/`
- `install.sh`
- `install.ps1`
- repository root metadata and release scripts outside the GPL tool directories

This boundary is intentional. GPL protects the compiler, package manager,
language server, and release-maintenance tools, while Kern libraries and
generated or user-written programs remain free from toolchain copyleft
obligations.

The canonical license texts live in:

- `LICENSES/GPL-3.0-or-later.txt`
- `LICENSES/MIT.txt`

Rust crates and extension packages should declare the matching SPDX license
identifier in their package metadata. New files should follow the license of
their containing package unless a more specific notice is present.
