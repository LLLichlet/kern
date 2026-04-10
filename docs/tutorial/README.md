# Kern Tutorial

This tutorial is the practical companion to the reference documents already in
this repository.

- [`docs/design.md`](../design.md) is the language design and semantics document.
- [`docs/style.md`](../style.md) records the current source-style guidance used in this repository.
- [`docs/kernc.md`](../kernc.md) is the compiler driver guide.
- [`docs/craft.md`](../craft.md) is the package-manager architecture and behavior guide.

This tutorial answers a different question: "How do I actually become productive
in Kern and in this repository?"

It is written for three audiences:

- someone learning Kern from scratch
- the project author who needs a stable mental model of the toolchain
- contributors who need to know where behavior actually lives in the codebase

## What Is Stable Today

The current `v0.6.7` tree already has a fairly solid core:

- the language model in `docs/design.md`
- the `kernc` compile/link workflow
- the compiler pipeline from parse to sema to lowering to LLVM
- the official library layering under `library/base`, `library/sys`, `library/rt`, and `library/std`
- the current `kern-lsp` editor loop

`craft` is already usable, but its policy and ecosystem surface still need to be
read with more care:

- the command surface exists: `check`, `lock`, `fetch`, `build`, `run`, `test`
- lockfiles, sources, workspace discovery, and script hooks are implemented
- the long-term package ecosystem and policy surface are still evolving

Treat `kernc` as the "must know" path and `craft` as the "usable but still
evolving" path.

## Recommended Reading Order

1. [Chapter 1: Language Tour](./01-language-tour.md)
2. [Chapter 2: Daily `kernc` Workflow](./02-kernc-workflow.md)
3. [Chapter 3: Projects With `craft`](./03-craft-workflow.md)
4. [Chapter 4: LSP And Editor Workflow](./04-lsp-and-editor.md)
5. [Chapter 5: Repository And Compiler Map](./05-project-map.md)

If you are the project author, read Chapters 2 and 5 immediately after Chapter
1. That sequence gives you the fastest path from "I can read Kern" to "I can
change the compiler with intent."

## Project Map In One Page

The repository splits cleanly into four layers:

- `compiler/`: the language implementation
- `tools/craft`: package graph, lockfile, build planning, execution
- `tools/lsp`: editor-facing analysis server
- `library/`: the official Kern libraries (`base`, `sys`, `rt`, `std`)

Inside `compiler/`, the mental model is:

```text
kern source
  -> lexer
  -> parser
  -> AST
  -> semantic analysis
  -> lowering / monomorphized AST
  -> LLVM IR / object
  -> linker
```

That pipeline is the backbone of the entire project. `kern-lsp` reuses the
front half of it for analysis. `craft` reuses `kernc` as an explicit backend
instead of becoming a compiler in disguise.

## A Good First Week Plan

1. Compile and run `examples/hello_world.rn` with `kernc`.
2. Read `examples/test_closure.rn` and `examples/anonymous_aggregates.rn`.
3. Read the `kernc` CLI tests under `compiler/kernc_cli/tests`.
4. Build a tiny `Craft.toml` package and exercise `craft check`, `lock`,
   `build`, and `run`.
5. Open the same package through `kern-lsp` and confirm diagnostics,
   completion, and navigation behave the way you expect.
6. Finish by reading the repository map chapter and tracing one concrete
   feature end to end.

That loop is enough to turn the project from "large and fuzzy" into "layered
and searchable."
