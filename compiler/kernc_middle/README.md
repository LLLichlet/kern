# kernc_middle

`kernc_middle` owns shared compiler facts produced after semantic checking and
consumed by later analysis, lowering, const-eval, and optimization layers.

This crate is deliberately not another expression IR. Its role is to hold
stable middle-layer data that multiple compiler phases can share without
depending on `kernc_sema` implementation details.

The initial boundary is typed node facts: source `NodeId`s mapped to inferred
types and selected semantic lowering facts. Future typed-body or control-flow
IR should grow here when it has one shared owner and more than one consumer.
