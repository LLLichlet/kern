---
title: "Tooling Model"
summary: "Understand the boundary between `craft`, `kernc`, `kern-lsp`, and the repository docs."
order: 3
---

Kern already has a real tooling split. If you learn that split early, the rest
of the toolchain becomes much easier to reason about.

## The Three Main Tools

### `craft`

`craft` is the package manager and build orchestrator.

It owns:

- `Craft.toml`
- package/workspace discovery
- dependency resolution
- lockfiles
- build plans
- running selected targets

As a practical rule, if the question is "which package or target am I building
or running?", `craft` is usually the right layer.

### `kernc`

`kernc` is the compiler and linker driver.

It owns:

- parsing and semantic analysis of one explicit source entry
- lowering, MIR construction, and code generation
- LLVM IR emission
- linker-input emission
- explicit link-only mode

As a practical rule, if the question is "what exact compile or link action
should happen?", `kernc` is the right layer.

### `kern-lsp`

`kern-lsp` is the language server. It reuses compiler analysis rather than
building a separate editor-only frontend.

As a practical rule, if the question is "how does the editor know what this
symbol means?", the answer should flow through the compiler analysis stack, not
through a second syntax engine with different semantics.

## `craft` Above `kernc`

The relationship between `craft` and `kernc` is straightforward:

- `craft` decides **what** to build
- `kernc` executes the explicit compile/link step for that decision

That boundary matters because Kern intentionally avoids turning the compiler
driver into a hidden package manager.

## Inspecting LLVM IR With `kernc`

The package layer is not the only useful layer. Sometimes you want to ask the
compiler directly what it is generating.

For a simple `main.rn`, you can run:

```bash
kernc --emit-llvm --runtime-entry rt --library-bundle std src/main.rn
```

That exact flow was validated while writing this guide against the same minimal
project used in the previous chapter.

The emitted IR is large because `std` and `rt` participate in the build, but
the command is still the right mental model:

- `kernc` takes explicit source input
- runtime policy stays explicit
- library bundle choice stays explicit
- LLVM IR emission is a driver mode, not a side effect hidden behind package tooling

## Where The Current Truth Lives

Right now the website is still being built out, so the repository docs remain
the authoritative references:

- language semantics: `docs/design.md`
- runtime/library model: `docs/runtime-architecture.md`
- compiler-driver behavior: `docs/kernc.md`
- package/build behavior: `docs/craft.md`

The website guide should teach the system, not compete with those documents by
silently drifting away from them.
