# `library/std`

`std` is the user-facing standard library.

It provides filesystem, IO, environment, process, message, synchronization,
terminal, testing, and time facilities on top of `base` and `sys`.

## Boundaries

- `std` may depend on `base` and `sys`.
- Raw OS/provider details stay in `sys`.
- Runtime startup and compiler-required fallbacks stay in `rt`.
- Freestanding primitives that do not need providers should stay in `base`.

## Documentation

From the repository root:

```sh
craft doc --project-path library/std
```
