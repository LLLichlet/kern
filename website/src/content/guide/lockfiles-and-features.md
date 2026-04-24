---
title: "Lockfiles And Features"
summary: "Treat `Craft.lock` as the canonical resolved graph, and treat CLI feature selection as an execution input layered on top."
order: 30
---

`craft` accepts feature-selection flags, but that does not mean feature
selection and lockfile state are the same thing.

The current implementation keeps those concerns separate.

## A Validated Example

The package used for validation had this feature section:

```toml
[features]
default = ["stable"]
stable = []
tls = []
simd = []
full = ["tls", "simd"]
```

Two `craft check --verbose` runs produced these feature summaries:

```text
features   dev, default=on, explicit=<none>
```

and:

```text
features   dev, default=off, explicit=full
```

Those came from these validated commands:

```bash
craft check --verbose --project-path /tmp/kern-site-lockfeat-vZ1Vxn
craft check --verbose --no-default-features --features full --project-path /tmp/kern-site-lockfeat-vZ1Vxn
```

## `Craft.lock` Stayed Stable

The same package was then checked twice:

```bash
craft check --project-path /tmp/kern-site-lockfeat-vZ1Vxn
craft check --no-default-features --features full --project-path /tmp/kern-site-lockfeat-vZ1Vxn
```

Both runs produced the same `Craft.lock` hash:

```text
85a12c56169686f156777f65af0e721d159e89c47cac12bd46a9db1d0b7b6842
```

The second run reported the lockfile as current.

That is exactly the model the current docs describe:

- `Craft.lock` records the canonical resolved package graph
- selected CLI feature sets are execution inputs, not new resolution identity

## Analysis Context Records The Feature Input

After the explicit-feature check run, the generated
`.craft/analysis.toml` contained:

```toml
profile = "dev"
default-features = false
features = ["full"]
```

That is a useful mental model:

- the analysis/build context records the selected execution input
- the lockfile remains the shared canonical graph artifact

## Declared Features Are Required

Current `craft` is strict about feature names.

This validated command:

```bash
craft check --features ghost --project-path /tmp/kern-site-lockfeat-vZ1Vxn
```

failed with:

```text
selected feature `ghost` is not declared in `[features]`
```

So feature names are not free-form toggles. They must exist in the manifest.

## Practical Rules

- use `[features]` to declare the feature vocabulary explicitly
- use `--features ...` and `--no-default-features` to choose an execution
  configuration
- use ordinary package-graph commands such as `craft check`, `craft build`, or `craft publish` to keep the canonical resolved graph snapshot synchronized

That separation is one of the reasons the toolchain stays understandable: lock
state is shared and stable, while feature selection stays a deliberate
invocation choice.
