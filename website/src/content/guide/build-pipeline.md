---
title: "Build Pipeline"
summary: "See how `craft` and `kernc` fit together, and when to drop down to compiler-driver level."
order: 8
---

By the time you can build and run a minimal package, you should also understand
what layer is doing what work.

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

This matters for documentation because the website should describe the current
pipeline honestly instead of flattening everything into a generic "frontend and
backend" story.

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

That exact driver flow was exercised while writing this guide against the same
minimal hello-style project used in the earlier chapters.

The output is large, but the command teaches the right lesson:

- source input is explicit
- runtime entry is explicit
- library bundle is explicit
- LLVM IR is a first-class compiler-driver output mode

## Why This Split Is Good

This separation keeps the toolchain understandable:

- package logic stays in the package tool
- compiler-driver logic stays in the compiler driver
- editor semantics can reuse compiler analysis instead of inventing a second frontend

That is a healthier base for a systems language than one giant tool that tries
to blur all responsibilities together.
