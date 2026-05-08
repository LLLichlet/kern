# kernc_flow

`kernc_flow` defines shared flow-analysis contracts that other compiler layers
consume without depending on the driver's internal flow implementation.

`kernc_flow` must stay below semantic checking implementation details. Flow
facts are keyed by shared compiler identities from `kernc_ty` and source node
IDs from `kernc_utils`, not by `kernc_sema` internals. Analysis logic should
move here as the typed-body boundary becomes explicit.
