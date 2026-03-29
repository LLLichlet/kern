# Contributing to Kern

First off, thank you for considering contributing to Kern! 

## Project Governance and Vision

Kern is fundamentally a personal project and founder-led. While community contributions, bug reports, and pull requests are incredibly valuable and deeply appreciated, the final decisions regarding language design, compiler architecture, and feature roadmaps rest entirely with the project founder. 

This model ensures a unified vision and prevents the language design from becoming fragmented. Before submitting a large Pull Request for a new feature, please open an Issue to discuss the design first to ensure it aligns with the project's direction.

## Development Setup

Kern is written in Rust. You will need the standard Rust toolchain installed.

1. Clone the repository:
```bash
git clone https://github.com/YOUR_USERNAME/kern.git
cd kern
```

2. Build the compiler:
```bash
cargo build
```

## Running Tests

Before submitting a Pull Request, please ensure all tests pass.

The active compiler regression suite is centered on the `kernc_cli` integration tests:
1. **Rust unit tests:** Kept close to the implementation inside the relevant `compiler/kernc_*` crate.
2. **CLI integration tests:** Located in [`compiler/kernc_cli/tests/`](compiler/kernc_cli/tests/). These tests compile and, where needed, execute temporary `.kr` programs against the real `kernc` binary.

To run all tests, simply execute:

```bash
cargo test -p kernc_cli --tests
```

### Adding a New Test

Add new integration coverage to the narrowest existing suite in `compiler/kernc_cli/tests/`.

- Reuse the shared harness in [`compiler/kernc_cli/tests/support/mod.rs`](compiler/kernc_cli/tests/support/mod.rs) instead of duplicating temporary-file or process-launch helpers.
- Keep compile-only checks and hosted runtime checks explicit in the test body.
- Prefer targeted regression tests for bug fixes, and suite-local helpers only when they genuinely encode behavior unique to that suite.

If a new area grows beyond a few related cases, split it into a new integration test file and document the new suite in [`compiler/kernc_cli/tests/README.md`](compiler/kernc_cli/tests/README.md).

## Commit Guidelines

We use [Conventional Commits](https://www.conventionalcommits.org/). Please format your commit messages accordingly:

* `feat:` A new feature
* `fix:` A bug fix
* `docs:` Documentation only changes
* `refactor:` A code change that neither fixes a bug nor adds a feature
* `test:` Adding missing tests or correcting existing tests

Example:
`fix(lower): correct type inference for early returns`
