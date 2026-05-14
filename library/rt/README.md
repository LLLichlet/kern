# `rt`

`rt` is the toolchain-owned runtime companion layer.

It is intentionally narrow. This layer exists to provide startup glue and the
minimal compiler-required runtime fallbacks that are still needed when no libc
implementation is linked.

## Current Layout

- `mod.kn`: root wiring for the `rt` layer
- `entry.kn`: platform-specific process entry shims and the handoff to
  `__kern_main_adapter`
- `memory_fallbacks.kn`: `memcpy`/`memmove`/`memset` fallback implementations
- `math_fallbacks.kn`: compiler-required math fallbacks when libc is absent

## Boundaries

- `rt` is injected only when runtime startup is selected
- `rt` is not a public prelude
- `rt` does not replace `base` or `std`
- the official `rt` package does not depend on `base` or `std`
- user-facing hosted facilities still belong in `std`
- hosted implementation details belong in `std.host`

In other words, `rt` owns startup and minimal runtime glue, not general-purpose
library APIs.

## Documentation

From the `library/` workspace root:

```sh
craft doc --project-path rt
```
