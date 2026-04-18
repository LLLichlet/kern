---
title: "Build Pipeline"
summary: "See how `craft` and `kernc` fit together, and when to drop down to compiler-driver level."
order: 16
---

By the time you can build and run a minimal package, it helps to understand
which layer is doing which work.

## The Short Version

The practical split is:

- `craft` owns packages, targets, and build orchestration
- `kernc` owns explicit compile/link actions

That is the same boundary reflected in the current repository docs and the
current implementation.

## The Compiler Pipeline

The current high-level compiler pipeline is:

```text
source
  -> lexer/parser -> AST
  -> semantic analysis
  -> Flow analysis
  -> MAST lowering
  -> MIR construction and optimization
  -> LLVM IR / object emission / linking
```

The website guide should describe this pipeline directly instead of flattening
it into a generic "frontend and backend" overview.

## When To Use `craft`

Start with `craft` when you want to:

- check a package
- build a target
- run a binary
- work inside a workspace/package graph

Examples:

```bash
craft check
craft build
craft run
```

## When To Use `kernc`

Drop to `kernc` when you want explicit driver behavior, such as inspecting LLVM
IR directly:

```bash
kernc --emit-llvm --runtime-entry rt --library-bundle std src/main.rn
```

That driver flow was exercised while writing this guide against the same
minimal hello-style project used in the earlier chapters.

The output is large, but the command still illustrates the driver boundary:

- source input is explicit
- runtime entry is explicit
- library bundle is explicit
- LLVM IR is a first-class compiler-driver output mode

## Why The Split Matters

This separation keeps the toolchain structure explicit:

- package logic stays in the package tool
- compiler-driver logic stays in the compiler driver
- editor semantics can reuse compiler analysis instead of requiring a separate frontend
