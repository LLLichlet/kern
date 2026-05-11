# Kern Examples

This directory is a small first-learn package. Each file is a standalone
example target managed by Craft.

Build every example:

```bash
craft build --project-path examples --examples
```

Run one example:

```bash
craft run --project-path examples --example hello_world
craft run --project-path examples --example collections
```

The examples are intentionally small:

- `hello_world.rn`: hosted program entry and formatted output.
- `basics.rn`: bindings, structs, functions, loops, and assertions.
- `control_flow.rn`: enums, `match`, and structural value patterns.
- `anonymous_aggregates.rn`: anonymous structs, unions, enums, and layout.
- `slices_and_iterators.rn`: slices, mutable slices, range iterators, and `for`.
- `string.rn`: allocation-explicit string building.
- `collections.rn`: `List` and common sequence helpers.
- `test_closure.rn`: function pointers and closures.
- `closure_heap_escape.rn`: manually storing a closure in allocator-owned memory.
- `sync.rn`: `Atomic[T]`, `SpinLock[T]`, and `Once`.
- `io_and_files.rn`: atomic file writes and whole-file reads.
- `void.rn`: zero-sized `void` and erased pointers.

These files are examples, not compiler test fixtures. Compiler and standard
library regressions should live under the Rust test suites instead.

The directory also contains standalone example packages:

- `limine-smoke/`: freestanding kernel package that builds a bootable Limine ISO.
- `limine-mkiso/`: hosted helper tool used by the Limine package.
