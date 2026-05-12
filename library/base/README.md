# `library/base`

`base` is the freestanding foundation library.

It contains runtime-independent primitives that every Kern build mode can use:
comparison traits, numeric traits, pointer and layout helpers, allocation
interfaces, options/results, hashes, ranges, strings, slices, lists, maps,
trees, synchronization primitives, generic IO helpers, and test assertions.

## Boundaries

- `base` must not depend on `std`, `prov`, or `rt`.
- Provider contracts belong in `prov`; hosted conveniences belong in `std`.
- User-facing hosted conveniences belong in `std`.
- Startup glue belongs in `rt`.

## Documentation

From the repository root:

```sh
craft doc --project-path library/base
```
