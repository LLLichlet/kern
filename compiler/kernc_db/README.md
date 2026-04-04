# kernc_db

`kernc_db` is Kern's small query-driven incremental engine.

It exists to support compiler incrementality without importing a larger
framework or hiding too much behavior behind implicit magic.

## Design Goals

`kernc_db` is intentionally simple.

It provides:

- `Input<K, V>` for externally supplied facts
- `Query<K, V>` for derived facts with tracked dependencies
- `Memo<K, V>` for cached staged computations
- revision tracking
- cycle detection with explicit query names

The goal is not to imitate Salsa feature-for-feature.
The goal is to give Kern a clear, controllable dependency engine that fits the
project's "healthy compiler" direction.

## Architectural Role

The current driver uses `kernc_db` to support:

- source override tracking
- cached frontend parsing
- staged structure/import/type artifacts
- body-only invalidation where possible

This keeps incrementality explicit in the compiler pipeline instead of scattering
ad-hoc caches through unrelated code.

## Why A Dedicated Engine

Kern wants query-driven compilation, but also wants the mechanics to stay easy
to inspect and reason about.

That means:

- dependency edges are recorded explicitly
- revisions change only when inputs actually change
- cycles surface as real diagnostics instead of hidden recursion failures
- the compiler can choose staged artifacts deliberately instead of relying on a
  fully opaque framework

## Relationship To `Flow`

`kernc_db` and `Flow` solve different problems:

- `kernc_db` answers when a fact must be recomputed
- `Flow` answers what control/dataflow facts a program body contains

Together they form the analysis side of Kern's architecture:

1. tracked inputs and staged queries in `kernc_db`
2. semantically checked program structure in `kernc_driver`
3. explicit CFG/dataflow facts in `Flow`
4. lowering into MAST only after analysis has done its job
