# Documentation Map

This document indexes the current Kern documentation set and clarifies which
documents should be treated as authoritative for different audiences.

It supports three different jobs:

- public documentation such as the repository README and language guide
- tool-facing reference material for `kernc`, `craft`, and `kern-lsp`
- implementation-facing notes for compiler/toolchain maintainers

The important rule is simple:

- public semantics and user-facing behavior should come from the `docs/` set
- crate README files should explain implementation boundaries, not define end-user policy

## Public Product Docs

These are the current public reference documents.

- [`README.md`](../README.md): default English repository front page, high-level language/toolchain overview, installation, and documentation entry points; the Simplified Chinese version is [`README.zh.md`](../README.zh.md)
- [`Nix.md`](../Nix.md): Nix and NixOS usage for installing Kern through flake configuration and entering the repository development shell
- [`docs/install.md`](./install.md): SDK installation, installed layout, offline installs, source builds, local archive packaging, and reproducibility checks
- [`docs/tutorial/`](./tutorial/README.md): default English guided tour through tools, language basics, core semantics, library usage, and freestanding entry points; the Simplified Chinese version is [`docs/tutorial/zh/README.md`](./tutorial/zh/README.md)
- [`docs/design.md`](./design.md): current language semantics and syntax
- [`docs/kernc.md`](./kernc.md): `kernc` CLI/driver behavior
- [`docs/craft.md`](./craft.md): package manager, lockfile, resolution, and build orchestration model
- [`docs/runtime-architecture.md`](./runtime-architecture.md): `base` / `rt` / `std` split and runtime/library policy
- [`docs/unix-distribution.md`](./unix-distribution.md): Unix host-tool distribution policy
- [`docs/windows-distribution.md`](./windows-distribution.md): Windows host-tool distribution policy
- [`docs/style.md`](./style.md): repository source-style guidance

## Implementation Docs

These documents explain how the current compiler/tooling implementation is
structured internally. They are valuable for maintainers, but they are not the
primary source for user-facing guide or tutorial copy.

- [`compiler/kernc_driver/README.md`](../compiler/kernc_driver/README.md): driver staging, `Flow`, incremental behavior, and analysis boundaries
- [`compiler/kernc_db/README.md`](../compiler/kernc_db/README.md): incremental query engine model
- [`compiler/kernc_lower/README.md`](../compiler/kernc_lower/README.md): lowering from semantic state into MAST
- [`compiler/kernc_mast/README.md`](../compiler/kernc_mast/README.md): MAST role and boundaries
- [`compiler/kernc_mir/README.md`](../compiler/kernc_mir/README.md): MIR role, verification, and optimization
- [`compiler/kernc_mir_lower/README.md`](../compiler/kernc_mir_lower/README.md): staged MAST -> MIR lowering boundary
- [`compiler/kernc_flow/README.md`](../compiler/kernc_flow/README.md): shared flow-analysis contracts
- [`compiler/kernc_mono/README.md`](../compiler/kernc_mono/README.md): monomorphization identities and metadata

## Tool Docs

These documents sit between public behavior and implementation detail.

- [`tools/craft/README.md`](../tools/craft/README.md): current `craft` surface and internal module index
- [`tools/lsp/README.md`](../tools/lsp/README.md): current `kern-lsp` feature surface, protocol coverage, and integration constraints
- `tools/kernup`: Rust SDK installer and future toolchain manager entry point
- `tools/kernworker`: Rust repository maintenance and CI worker entry point

## Source Of Truth By Topic

When writing tutorials or guide material, prefer these sources:

- language semantics and syntax: [`docs/design.md`](./design.md)
- runtime and library layering: [`docs/runtime-architecture.md`](./runtime-architecture.md)
- compiler CLI behavior: [`docs/kernc.md`](./kernc.md)
- package manager behavior: [`docs/craft.md`](./craft.md)
- installation behavior and SDK layout: [`docs/install.md`](./install.md)
- platform release/distribution constraints: [`docs/unix-distribution.md`](./unix-distribution.md) and [`docs/windows-distribution.md`](./windows-distribution.md)
- implementation architecture details: the relevant crate README files under [`compiler/`](../compiler/) and tool README files under [`tools/`](../tools/)
