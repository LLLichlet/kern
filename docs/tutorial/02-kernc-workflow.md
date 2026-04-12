# Chapter 2: Daily `kernc` Workflow

`kernc` is the stable center of the current toolchain. If you can drive `kernc`
comfortably, you can always fall back to a direct and debuggable build path.

## What `kernc` Is Responsible For

`kernc` does four things:

- parse and analyze one explicit source entry
- lower and codegen it
- optionally emit LLVM IR or an object file
- optionally invoke the system linker

It is not a package manager. That separation matters throughout the project.

## The Four Driver Modes

The CLI exposes four mutually exclusive modes:

1. default compile-and-link mode
2. `-c` for compile only
3. `--emit-llvm[=stage]` for LLVM IR output
4. `--link-only` for a pure link step

Examples:

```bash
kernc --library-bundle std --runtime-entry crt --runtime-libc yes examples/hello_world.rn -o hello
```

```bash
kernc -c --library-bundle std --runtime-entry crt --runtime-libc yes examples/hello_world.rn -o hello.o
```

```bash
kernc --emit-llvm --library-bundle std --runtime-entry crt --runtime-libc yes examples/hello_world.rn
```

```bash
kernc --emit-llvm=optimized -O2 --library-bundle std --runtime-entry crt --runtime-libc yes examples/hello_world.rn
```

```bash
kernc --link-only --link-input hello.o -o hello
```

## The Flags You Will Use Most

### Runtime and library flags

The main knobs are:

- `--library-bundle <none|base|std>`
- `--runtime-entry <none|rt|crt>`
- `--runtime-libc <yes|no>`

`--library-bundle std` maps the official `std` root alias if one is not already
provided manually.

The official shipped library roots are:

- `base`: foundation facilities
- `sys`: operating-system and provider boundaries
- `rt`: startup/runtime glue
- `std`: high-level user-facing facilities

`kernc` adds official library aliases only when you ask for a library bundle.
This is alias wiring, not a prelude. `rt` is not added by bundle selection
alone; it is added only when a runtime entry contract is active. That `rt`
companion-root wiring does not also add `base` or `sys`.

The official library lookup order is:

1. `KERN_STD_PATH`
2. `KERN_BASE_PATH`
3. `KERN_SYS_PATH`
4. `KERN_RT_PATH`

Each root then falls back to a path relative to the current executable and finally to `library/<name>` in the repository layout.

These flags are orthogonal. Library availability, program-entry semantics, and libc linkage are configured independently. `sys`/`rt` implementation choice is handled through ordinary module paths or packages.

### `--module-path name=path`

Maps a module root to a concrete directory. This is the key escape hatch when
you want full explicit control.

```bash
kernc --module-path std=./library/std app.rn
```

### `--module-interface-path name=path` and `--metadata-output`

These are for module interface snapshots:

- `--metadata-output <dir>` writes a metadata export tree
- `--module-interface-path name=path` imports a metadata root

This matters when you want explicit interface-style builds without teaching
`kernc` package-manager behavior.

## Three Useful Workflows

### 1. Hosted User-Space Experiment

Use this when you are exploring language behavior quickly on your own machine:

```bash
kernc --library-bundle std --runtime-entry crt --runtime-libc yes scratch.rn -o scratch
./scratch
```

### 2. Freestanding Or Kernel-Oriented Build

Use this when you want to stay close to bare-metal assumptions:

```bash
kernc -c kernel.rn -o kernel.o
kernc --link-only --link-input kernel.o --entry-symbol boot_main --link-arg -nostdlib -o kernel
```

Splitting compile and link is especially useful when debugging symbol export,
entry-point, or linker-argument issues.

### 3. Inspecting Codegen

When you are unsure how a feature lowers, emit raw LLVM IR first:

```bash
kernc --emit-llvm feature_probe.rn
```

That is often the fastest way to answer "is the front end wrong, or is lowering
/ codegen wrong?"

When you need to inspect LLVM's own pass effects instead, ask for a later stage:

```bash
kernc --emit-llvm=optimized -O2 feature_probe.rn
```

## Conditional Compilation Inputs

`kernc` accepts `--define key=value` pairs that feed frontend pruning and conditions.

```bash
kernc --define board=qemu --define debug_mode=true app.rn
```

The driver also injects some condition values itself, including:

- `runtime_entry`
- `library_bundle`
- `libc`
- `crt_startup`
- `rt_role`

That allows the standard library and user code to prune configuration-specific
items without inventing hidden global state.

## A Good Debugging Order

When something feels wrong, debug in this order:

1. run `kernc` directly instead of going through higher-level tooling
2. reduce the case to one entry `.rn` file
3. switch to `-c` or `--emit-llvm`
4. make module aliases explicit with `--module-path`
5. only after that bring `craft` back into the picture

This order keeps the problem close to the compiler boundary.

## Common Mistakes

- passing a source file together with `--link-only`
- assuming `--library-bundle std` also implies hosted startup or libc linkage
- assuming `let mut` grants write permission through every access path
- forgetting that `kernc` currently handles one explicit source entry at a time

If you are learning the project deeply, `kernc` should become your default
probe. It is the clearest path from source text to compiler behavior.
