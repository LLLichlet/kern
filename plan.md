# Kern Follow-up Plan

This file records the post-0.8.2 design work that should be handled as dedicated follow-up tasks. It is not a release note and does not carry the temporary bug list that has already been triaged.

## Full constexpr Upgrade

The current constexpr evaluator is good enough for constant folding, `const fn`, const generics, and hosted `build.kn` execution, but it is still an AST-level evaluator that grew out of constant-expression folding. The next step should be a real interpreter core instead of more local syntax patches.

Direction:

1. Introduce a formal interpreter runtime model: frames, local slots, allocations, places, operands, and projections.
2. Keep values tied to type and layout information so `ConstValue` does not need to recover semantics from surrounding context.
3. Model pointers, slices, ranges, string/static data, temporary aggregates, and host-returned values explicitly.
4. Prefer interpreting typed lowered IR over parser AST in the long term, reducing drift between constexpr and normal compilation.
5. Treat `build.kn` as a hosted interpreter capability, not as a separate language path with special cases.
6. Preserve step limits, call stacks, diagnostic context, and host-effect boundaries so interpreted Kern remains controlled.

rustc/Miri is worth studying for structure, but Kern should not copy Rust's full borrow/provenance complexity. The goal is a unified interpreter architecture that fits Kern's own type semantics.

## Print And IO Semantics

The current `.fmt()`, `.print()`, and `.println()` flow is pleasant, but `read`, `write`, formatted output, and formatted input still need a formal design boundary. This should be settled as an API design task rather than through small piecemeal additions.

Topics to decide:

1. Whether object methods and free functions should coexist, for example keeping `"x={}".fmt(.{x}).println()` while also offering `println("x={}", .{x})` for lower migration friction.
2. Whether `Writer`, `Reader`, `Sink`, and `Source` responsibilities need clearer naming and separation, avoiding confusing forms such as `stdin().write()`.
3. How formatted write should express errors, allocator policy, and no-allocation guarantees.
4. Whether files, stdout/stderr, memory buffers, and string construction should share one formatting core.
5. Whether formatted read or scan-style APIs are needed, and how they should handle failure, partial reads, locale, and allocation.

The goal is for CLI examples, file I/O, standard output, and library-internal formatting to use one clear semantic model instead of several similar but inconsistent interfaces.

## LSP Issues

The LSP now covers the basic editing workflow, but real projects still expose friction that needs systematic performance and stability work.

Focus areas:

1. Inlay hints, hover, diagnostics, and semantic tokens need a more reliable degradation strategy for broken source. If a span is not trustworthy, hiding the hint is better than showing misplaced information.
2. Dirty documents and saved artifacts need another cache-consistency pass so unsaved edits do not surface stale, slow, or misplaced results.
3. Diagnostic feedback is still not fast enough. `craft check` is quick, but LSP squiggles, unused diagnostics, and hover details can lag, which points to scheduling, caching, debounce, or lock contention.
4. Unused checks should run as cancelable tasks and discard old results when the input changes again.
5. Hover should prefer existing artifacts or cache entries, returning a lightweight fallback when full analysis is unavailable instead of blocking on complete analysis.
6. VS Code behavior for multiline strings, line continuation, snippet suppression, and highlighting needs continued regression coverage.
7. Code lens, run/build targets, generated source aliases, and workspace package resolution should keep regression tests to avoid drift in ecosystem projects.

The first step should be LSP timings/tracing that separates open/change-to-diagnostics, hover, inlay hints, and semantic tokens. Optimization order should come from that data.
