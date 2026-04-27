# `library/sys`

`sys` is the system and provider boundary layer.

It owns raw operating-system calls, platform metadata, page allocation, process
and filesystem provider contracts, and terminal/time boundaries that higher
layers build on.

## Boundaries

- `sys` may depend on `base`.
- `sys` must not depend on `std`.
- Higher-level formatting, path ergonomics, allocation policy, and user-facing
  wrappers belong in `std`.
- Startup glue belongs in `rt`.

## Documentation

From the repository root:

```sh
craft doc --project-path library/sys
```
