# 11. Next Steps

English | [简体中文](../zh/11-下一步.md)

After the tour, choose the next path based on what you want to build.

## Continue Learning The Language

Suggested reading order:

1. [`examples/basics.rn`](../../../examples/basics.rn)
2. [`examples/control_flow.rn`](../../../examples/control_flow.rn)
3. [`examples/slices_and_iterators.rn`](../../../examples/slices_and_iterators.rn)
4. [`examples/anonymous_aggregates.rn`](../../../examples/anonymous_aggregates.rn)
5. [`examples/test_closure.rn`](../../../examples/test_closure.rn)

Then read [`design.md`](../../design.md). It is the main public reference for
current syntax and semantics.

## Write Hosted Tools

Focus on:

- [`std.io`](../../../library/std/io/init.rn): stdout, stderr, and `Printable`.
- [`std.fs`](../../../library/std/fs): paths, files, and directories.
- [`std.proc`](../../../library/std/proc): arguments, process information, and shell capture.
- [`std.env`](../../../library/std/env): environment variables.
- [`std.time`](../../../library/std/time/init.rn): time and sleep.

Examples:

- [`examples/io_and_files.rn`](../../../examples/io_and_files.rn)
- [`examples/collections.rn`](../../../examples/collections.rn)

## Write Containers, Parsers, Or Libraries

Focus on `base`:

- [`base.coll`](../../../library/base/coll/init.rn): `List`, `String`, `Map`, `Tree`, ranges, and slice helpers.
- [`base.mem.alloc`](../../../library/base/mem/alloc/init.rn): allocator traits, GPA, and arena.
- [`base.io`](../../../library/base/io/init.rn): `Read`, `Write`, `Formatable`, and formatting.
- [`base.num`](../../../library/base/num/init.rn): numeric constants and parsing.

Larger packages in the Kern ecosystem, such as JSON or bit-level I/O libraries,
should follow the same `base` boundaries: allocator policy stays explicit,
borrowing and ownership are visible in the API, and tests cover the main
workflow before publishing.

## Write Kernels Or Freestanding Programs

Focus on:

- [`runtime-architecture.md`](../../runtime-architecture.md)
- the freestanding package sections in [`craft.md`](../../craft.md)
- the freestanding `_start` and linker-script sections in [`kernc.md`](../../kernc.md)
- [`examples/limine-smoke`](../../../examples/limine-smoke)

If your project supplies custom platform or runtime layers, treat them as
ordinary module/package boundaries. Do not assume the compiler provides hidden
platform roots for those layers.

## Work On Kern Itself

Start with these toolchain documents:

- [`compiler/kernc_driver/README.md`](../../../compiler/kernc_driver/README.md)
- [`compiler/kernc_db/README.md`](../../../compiler/kernc_db/README.md)
- [`compiler/kernc_lower/README.md`](../../../compiler/kernc_lower/README.md)
- [`compiler/kernc_mir/README.md`](../../../compiler/kernc_mir/README.md)
- [`tools/craft/README.md`](../../../tools/craft/README.md)
- [`tools/lsp/README.md`](../../../tools/lsp/README.md)
