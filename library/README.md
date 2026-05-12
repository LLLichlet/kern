# Kern Official Libraries

This workspace contains the toolchain-owned Kern libraries.

The libraries are regular Craft packages so their package metadata, dependency
graph, and native documentation can be inspected with Craft:

```sh
craft doc --project-path library
```

## Packages

- `base`: freestanding primitives, traits, memory helpers, collections, and
  numeric utilities.
- `std`: user-facing facilities built on `base`, with hosted implementation
  owned internally by `std.host`.
- `rt`: startup glue and minimal runtime fallbacks used by selected runtime
  entry modes.

The compiler and Craft resolve official libraries from this workspace root.
Set `KERNLIB_PATH` to point at an external copy of this workspace; SDK installs
place the same workspace at `lib/kern`. The `base`, `rt`, and `std` package
paths are derived from that single root instead of being configured as
independent library roots.
