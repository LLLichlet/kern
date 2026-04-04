# Chapter 5: Repository And Compiler Map

This chapter is for the "I need to move the project forward myself" phase.

If you only remember one thing, remember this: the repository is large, but the
responsibility boundaries are actually clean.

## Top-Level Layout

```text
compiler/     language implementation
tools/craft/  package manager and builder
tools/lsp/    language server
library/std/  standard library
library/toml/ independent package example
examples/     small language probes
docs/         design and user-facing documents
```

## The Compiler Pipeline

The driver in `compiler/kernc_driver` orchestrates the main pipeline:

```text
kernc_lexer
  -> kernc_parser
  -> kernc_ast
  -> kernc_sema
  -> kernc_lower
  -> kernc_mast
  -> kernc_codegen
  -> link step
```

That is not just conceptual. The driver literally follows that shape:

- analyze source into typed semantic state
- lower into monomorphized form
- codegen to LLVM-backed artifacts
- link if requested

## Where To Change What

### Syntax And Parsing

Start in:

- `compiler/kernc_lexer`
- `compiler/kernc_parser`
- `compiler/kernc_ast`

Typical tasks:

- add a token
- change a grammar production
- store new syntax on AST nodes

### Type Rules And Name Resolution

Start in:

- `compiler/kernc_sema/src/checker`
- `compiler/kernc_sema/src/passes`
- `compiler/kernc_sema/src/ty`

Typical tasks:

- expression typing
- coercion and explicit conversion rules
- trait lookup
- constant evaluation
- module/import behavior

### Lowering And Runtime Representation

Start in:

- `compiler/kernc_lower`
- `compiler/kernc_mast`

Typical tasks:

- monomorphization behavior
- closure lowering
- enum physical representation
- trait object / vtable lowering
- defer or control-flow lowering

### LLVM Code Generation

Start in:

- `compiler/kernc_codegen`

Typical tasks:

- LLVM IR shape
- inline assembly plumbing
- calling convention details
- platform-specific codegen bugs

### Driver, CLI, And Linking

Start in:

- `compiler/kernc_driver`
- `compiler/kernc_cli`
- `compiler/kernc_utils`

Typical tasks:

- CLI flags
- output mode selection
- linker command construction
- diagnostics and source mapping
- metadata export/import

### Package Manager

Start in:

- `tools/craft/src/manifest.rs`
- `tools/craft/src/graph.rs`
- `tools/craft/src/elaborate.rs`
- `tools/craft/src/lockfile.rs`
- `tools/craft/src/build_plan.rs`
- `tools/craft/src/execute.rs`
- `tools/craft/src/script.rs`

Typical tasks:

- manifest schema changes
- workspace/package graph rules
- lockfile freshness logic
- source policy
- `craft.rn` / `build.rn` host behavior
- action planning and execution

### Language Server

Start in:

- `tools/lsp/src/analysis.rs`
- `tools/lsp/src/analysis/*`
- `tools/lsp/src/server.rs`
- `tools/lsp/src/protocol.rs`

Typical tasks:

- diagnostics adaptation
- navigation features
- semantic tokens
- rename behavior
- protocol capability negotiation

## Three Reliable Reading Strategies

### 1. Follow A User Command

Example: start at `kernc_cli/src/main.rs`, then step into the driver, then into
the specific phase that interests you.

### 2. Follow A Test

The `compiler/kernc_cli/tests` directory is a very good map of real language
behavior. When you are unsure how a feature is meant to behave, the tests are
usually faster to read than the whole implementation.

### 3. Follow A Data Structure

If you want to understand the shape of the compiler, track the data:

- tokens
- AST nodes
- semantic definitions and types
- MAST nodes
- LLVM objects

This is often more effective than reading files in directory order.

## Suggested Founder Reading Order

1. `compiler/kernc_cli/src/main.rs`
2. `compiler/kernc_driver/src/compiler.rs`
3. `compiler/kernc_sema/src/checker`
4. `compiler/kernc_lower/src/lib.rs`
5. `compiler/kernc_codegen/src/codegen.rs`
6. `tools/craft/src/cli.rs`
7. `tools/lsp/src/analysis.rs`

That order gives you the shortest route to "I know where to cut when I want to
change something."

## Final Advice

Do not try to memorize the whole tree.

Instead, lock in these boundary rules:

- `kernc` compiles explicit inputs
- `craft` plans graphs and drives `kernc`
- `kern-lsp` reuses compiler analysis
- `std` is ordinary Kern source, not compiler magic

Once those boundaries are stable in your head, the rest of the project becomes
much easier to navigate.
