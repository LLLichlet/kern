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
3. `--emit-llvm` for LLVM IR output
4. `--link-only` for a pure link step

Examples:

```bash
kernc --use-std --link-profile hosted examples/hello_world.rn -o hello
```

```bash
kernc -c --use-std examples/hello_world.rn -o hello.o
```

```bash
kernc --emit-llvm --use-std examples/hello_world.rn
```

```bash
kernc --link-only --link-input hello.o -o hello
```

## The Flags You Will Use Most

### `--use-std`

Injects the `std` module alias if one is not already provided manually.

The lookup order is:

1. `KERN_STD_PATH`
2. a path relative to the current executable
3. `library/std` in the repository layout

### `--link-profile`

Selects default link behavior:

- `kern`
- `freestanding`
- `hosted`
- `none`

This is separate from language semantics. It is driver policy.

### `-M name=path`

Maps a module root to a concrete directory. This is the key escape hatch when
you want full explicit control.

```bash
kernc -M std=./library/std app.rn
```

### `-I name=path` and `--emit-kmeta`

These are for module interface snapshots:

- `--emit-kmeta <dir>` writes a kmeta export tree
- `-I name=path` imports a kmeta root

This matters when you want explicit interface-style builds without teaching
`kernc` package-manager behavior.

## Three Useful Workflows

### 1. Hosted User-Space Experiment

Use this when you are exploring language behavior quickly on your own machine:

```bash
kernc --use-std --link-profile hosted scratch.rn -o scratch
./scratch
```

### 2. Freestanding Or Kernel-Oriented Build

Use this when you want to stay close to bare-metal assumptions:

```bash
kernc -c --use-std --link-profile kern kernel.rn -o kernel.o
kernc --link-only --link-profile kern --link-input kernel.o -o kernel
```

Splitting compile and link is especially useful when debugging symbol export,
entry-point, or linker-argument issues.

### 3. Inspecting Codegen

When you are unsure how a feature lowers, emit LLVM IR first:

```bash
kernc --emit-llvm feature_probe.rn
```

That is often the fastest way to answer "is the front end wrong, or is lowering
/ codegen wrong?"

## Conditional Compilation Inputs

`kernc` accepts `-D key=value` pairs that feed frontend pruning and conditions.

```bash
kernc -D board=qemu -D debug_mode=true app.rn
```

The driver also injects some condition values itself, including:

- `link_profile`
- `hosted`
- `libc`
- `kern_rt`

That allows the standard library and user code to prune configuration-specific
items without inventing hidden global state.

## A Good Debugging Order

When something feels wrong, debug in this order:

1. run `kernc` directly instead of going through higher-level tooling
2. reduce the case to one entry `.rn` file
3. switch to `-c` or `--emit-llvm`
4. make module aliases explicit with `-M`
5. only after that bring `craft` back into the picture

This order keeps the problem close to the compiler boundary.

## Common Mistakes

- passing a source file together with `--link-only`
- assuming `--use-std` also implies hosted linking
- assuming `let mut` grants write permission through every access path
- forgetting that `kernc` currently handles one explicit source entry at a time

If you are learning the project deeply, `kernc` should become your default
probe. It is the clearest path from source text to compiler behavior.
