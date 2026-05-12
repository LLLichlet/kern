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
- `prov`: provider contracts built on `base`.
- `std`: user-facing facilities built on `base` and `prov`, with hosted implementation owned internally.
- `rt`: startup glue and minimal runtime fallbacks used by selected runtime
  entry modes.

The compiler and Craft still resolve these libraries through the official
library paths (`KERN_BASE_PATH`, `KERN_PROV_PATH`, `KERN_STD_PATH`, and
`KERN_RT_PATH`, or the SDK layout). The Craft manifests document the same
sources rather than replacing that toolchain resolution contract.
