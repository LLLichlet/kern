# `library/std`

`std` is the user-facing standard library.

It provides filesystem, IO, environment, process, message, synchronization,
terminal, testing, and time facilities on top of `base`.

## Boundaries

- `std` may depend on `base`.
- Hosted OS details stay inside `std.host`.
- Runtime startup and compiler-required fallbacks stay in `rt`.
- Freestanding primitives should stay in `base`.

## Documentation

From the repository root:

```sh
craft doc --project-path library/std
```
