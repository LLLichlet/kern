# kernc_mir_lower

`kernc_mir_lower` owns the staged lowering boundary from MAST into Kern MIR.

This crate exists so `kernc_mir` can converge toward MIR definition,
verification, and optimization, while lowering policy and source-IR adaptation
live in a separate layer.

This crate is responsible for either:

- lifting MAST into first-class MIR
- or failing immediately when a source form has not been modeled yet

It does not preserve opaque source-level fallback nodes inside MIR.
