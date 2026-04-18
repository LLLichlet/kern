---
title: "Tooling Model"
summary: "Understand the boundary between `craft`, `kernc`, `kern-lsp`, and the repository docs."
order: 3
---

This chapter introduces the current boundary between `craft`, `kernc`,
`kern-lsp`, and the repository docs.

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

If the question is "which package or target am I building or running?",
`craft` is usually the right layer.

### `kernc`

`kernc` is the compiler and linker driver.

It owns:

- parsing and semantic analysis of one explicit source entry
- lowering, MIR construction, and code generation
- LLVM IR emission
- linker-input emission
- explicit link-only mode

If the question is "what exact compile or link action should happen?",
`kernc` is the right layer.

### `kern-lsp`

`kern-lsp` is the language server. It reuses compiler analysis rather than
building a separate editor-only frontend.

If the question is "how does the editor know what this symbol means?", the
answer flows through the compiler analysis stack rather than a separate
editor-only syntax engine.

## `craft` Above `kernc`

The relationship between `craft` and `kernc` is straightforward:

- `craft` decides **what** to build
- `kernc` executes the explicit compile/link step for that decision

This boundary keeps package planning separate from compiler-driver behavior.

## Inspecting LLVM IR With `kernc`

Sometimes it is useful to ask the compiler directly what it is generating.

For a simple `main.rn`, you can run:

```bash
kernc --emit-llvm --runtime-entry rt --library-bundle std src/main.rn
```

This flow was validated while writing the guide against the same minimal
project used in the previous chapter.

The emitted IR is large because `std` and `rt` participate in the build. The
command still illustrates the driver boundary clearly:

- `kernc` takes explicit source input
- runtime policy stays explicit
- library bundle choice stays explicit
- LLVM IR emission is a driver mode

## Where The Current Truth Lives

Right now the website is still being built out, so the repository docs remain
the authoritative references:

- language semantics: `docs/design.md`
- runtime/library model: `docs/runtime-architecture.md`
- compiler-driver behavior: `docs/kernc.md`
- package/build behavior: `docs/craft.md`

The guide should stay aligned with those documents.
