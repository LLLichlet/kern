---
title: "Guide Overview"
summary: "How this guide relates to the current repository docs and what the first chapters cover."
order: 1
---

This guide is the current website learning path for Kern.

The current split is now sharper:

- the website guide is the onboarding path
- the website reference covers normal `craft` and `kernc` usage directly
- repository docs keep the deep design and maintainer-facing material

The intended long-term split is:

- guide pages for teaching and onboarding
- reference pages for precise language/tool behavior
- implementation docs in the repository for maintainers

## What The First Chapters Cover

The first guide chapters focus on the minimum a new user needs in order to
become productive without hiding the real toolchain model:

1. create and run a minimal package with `craft`
2. understand the boundary between `craft`, `kernc`, and `kern-lsp`
3. get a first look at Kern source forms that actually compile today

## Validation Policy

The examples in the current early chapters are not just illustrative snippets.
They were validated against the current repository toolchain while writing the
guide:

- a minimal `Craft.toml + src/main.rn` package was built and run with `craft`
- a small language sample using `struct`, `enum`, `match`, and formatting was
  also built and run successfully
- `kernc --emit-llvm` was exercised against the same minimal project to confirm
  the compiler-driver walkthrough

As the guide grows, the same rule should stay in place: tutorial material
should come from code we have actually run, not just code that looks plausible.
