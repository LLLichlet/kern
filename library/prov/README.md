# `library/prov`

`prov` contains provider contracts shared by hosted and freestanding
implementations.

It is not a hosted implementation package. Platform shims, syscalls, process
startup policy, and host allocation live in consumers such as `std.host` or in
kernel-provided packages.

Layering:

- `prov` may depend on `base`.
- `prov` must not depend on `std` or `rt`.
- `base` must not depend on `prov`.
