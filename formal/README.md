# Kern Formal Models

This directory contains Lean models for small soundness-critical parts of Kern.
The first models formalize the fake-reflection and method-lookup boundaries
around traits, supertraits, associated-type projections, const generics, and
generics.  It now also includes small models for pattern-match coverage,
scalar range coverage, and lowering order.

Run it with:

```sh
cd formal
lake build
```

The model is intentionally smaller than the compiler. Its job is to pin down
the invariants that implementation paths must share:

- global impl projection only runs for fully concrete obligations
- proof search and projection normalization use the same specificity frontier
- shadowed blanket impls cannot provide associated-type equalities
- bare trait objects do not infer missing associated bindings
- supertrait upcasts retain only validated inherited associated bindings
- type and const generic variables keep projections open
- method lookup follows compiler source order and stops at the first unique or
  ambiguous source
- impl/default methods use the same maximal-specificity frontier, and open
  type/const generic impl arguments do not expose methods
- generic associated-type arguments instantiate the already selected associated
  family target, rather than triggering another impl/projection selection
- const-generic expression identity is conservative: concrete expressions may
  fold to value identity, while open expressions keep their symbolic shape and
  are not reverse-solved during direct inference
- trait default method lowering preserves owner trait arguments and hidden
  receiver `Self`, and vtable slots prefer explicit impl methods over defaults
- monomorphization identity includes both type and const generic arguments, and
  cache misses enqueue exactly one finite pending function instantiation
- coherence and orphan rules treat const generic impl heads as real identity
  components and reject foreign trait impls without a local target anchor
- kmeta source-snapshot imports preserve const-generic impl heads, trait
  arguments, associated bindings, and imported package locality
- kmeta publication materializes linkable trait impl method bodies, and kmeta
  consumption treats imported no-body methods as external declarations
- match exhaustiveness and unreachable-pattern checks use the same constructor
  interpretation that lowering uses for ordered first-match dispatch; omitted
  struct pattern fields are wildcards, while opaque user patterns remain runtime
  tests and do not make a match exhaustive
- match arm alternatives share one body scope only when their canonical binding
  shapes agree, including `Pattern[T]::Bind`-derived user-pattern bindings
- `let ... else` arm blocks cover the complement of the primary success
  pattern; the checker seeds coverage with the success pattern before checking
  failure arms and reports the first still-uncovered failure witness
- scalar match coverage clips value/range patterns to the target finite domain,
  merges overlapping or adjacent intervals, treats empty intervals as
  unreachable, and reports the first uncovered scalar witness
- named-to-anonymous aggregate decay uses the same instantiated field shape
  that lowering rewrites by name; native structs compare structural field sets,
  while `extern struct` decay preserves ABI-visible declaration order
- closure callback identity keeps capture environments in closure-state layout,
  instantiates generic and const-generic function-item signatures before `&Fn`
  matching, and keys generated callback adapters by the full function item

## Compiler Correspondence

`KernFormal.FakeRefl` tracks the current compiler model at these points:

- `Projection` corresponds to `TypeKind::Projection { target, trait_def_id,
  trait_args, assoc_def_id, assoc_args }` in `compiler/kernc_ty/src/lib.rs`.
- `TraitRef` corresponds to `TypeKind::TraitObject(def_id, args,
  assoc_bindings)`.
- `Ty.containsUnresolved`, `GenericArg.containsUnresolved`, and
  `Projection.isConcrete` correspond to the concrete gates in
  `SemaContext::projection_is_fully_concrete` and
  `ExprChecker::projection_assoc_from_global_impls`.
- `ConstArg.containsUnresolved`, `ConstArg.occurs`, and `matchConstArg?`
  correspond to `ConstGeneric::{Value, Param, Expr}`,
  `TypeRegistry::const_generic_contains_params`, and the const-generic occurs
  check in `match_available_const_generic_against_requirement`.
- `normalizeProjectionFromCompilerSources?` mirrors the source order used by
  expression inference: explicit trait-object hierarchy bindings first,
  active environment bounds second, and global impl projection last.
- `maximalCandidates` and `uniqueMaximal?` model the specificity frontier used
  by `collect_specificity_maximal_trait_impl_head_candidates` and
  `collect_specificity_maximal_projection_candidates`.
- `selectByReplacement?` models the older replacement-style selection shape in
  expression inference. Keep it equivalent to the maximal-frontier model before
  relying on global impl projection facts.
- `traitViewsEquivalent`, `mergeTraitViews?`, and `mergeTraitViewPaths?` model
  the diamond-supertrait check in `trait_object_view_from_hierarchy_inner` and
  `target_trait_views_equivalent`: multiple paths to the same target trait must
  agree on the target trait's declared associated bindings, or the hierarchy
  query returns no view.

`KernFormal.MethodLookup` tracks the current compiler model at these points:

- `lookupBySources` mirrors `MemberQuery::resolve_named_method_in_type` in
  `compiler/kernc_sema/src/query/named.rs`: trait-object receivers, inherent
  impl methods, active generic bounds, projection-associated bounds, ordinary
  impl methods, trait default methods, and invalid self-referential impl errors
  are tried in that order.
- `resolveMethodSource`, `maximalMethodCandidates`, and
  `uniqueMaximalMethod?` model
  `collect_specificity_maximal_impl_method_candidates_with_filter` and
  `collect_specificity_maximal_trait_default_method_candidates`.
- `MethodCandidate.argsResolved` models the final
  `impl_generic_args_fully_resolved` gate in `resolve_impl_applicability`,
  including symbolic const-generic expressions.
- `selectMethodByReplacement?` is a regression oracle for the old
  pick-one-candidate shape: it must not be used for impl/default method lookup,
  because incomparable where-bound methods and default methods have to remain
  ambiguous.

`KernFormal.AssocSubst` tracks the current compiler model at these points:

- `instantiateAssocTarget?` mirrors
  `SemaContext::instantiate_assoc_projection_target`: assoc generic arguments
  are substituted into the associated family target selected from an object,
  bound, or impl.
- `substituteAssocPlaceholders?` mirrors the `TypeKind::Associated` arm of
  `substitute_associated_types`, including the rule that `Assoc[Arg]`
  instantiates a stored family binding before the type is used in a contract or
  inherited method signature.
- `projectionArgsDirectMatch?` mirrors the direct projection-argument matching
  in call-signature inference: a plain generic parameter may bind, but symbolic
  const expressions such as `N + 1` are not inverted to solve `N`.

`KernFormal.ConstExprIdentity` tracks the current compiler model at these
points:

- `ConstExpr`, `ConstKey`, and `foldConst` mirror `ConstGeneric`,
  `ConstExprKind`, and `TypeRegistry::fold_const_generic`: concrete integer
  expressions fold to values, while params and non-foldable expressions remain
  symbolic.
- Division by zero remains an expression instead of folding, matching the
  compiler path that leaves diagnostics to the checker.
- `directInfer?` mirrors direct const-generic inference: a plain const param can
  bind to a concrete key, but `N + 1` is not algebraically inverted to solve
  `N`.
- `canonicalMonoKey` records the emitted-item identity boundary: folded
  expressions share value identity, but open expressions such as `N + 1` remain
  distinct from `N` and must be preserved through source/import snapshots.

`KernFormal.DefaultVtable` tracks the current compiler model at these points:

- `buildVtableMethodEntry` mirrors the method-entry part of
  `LoweringContext::build_and_inject_vtable_global`: explicit impl methods are
  installed before default bodies are considered.
- `defaultFunctionArgs` mirrors `trait_default_function_args` and
  `resolve_trait_default_method_target`: the lowered `FnDef` arguments are the
  owner trait's generic arguments followed by the matched receiver type as the
  hidden default-method `Self`.
- `buildVtable` records the vtable layout invariant that transitive supertrait
  vtable pointers are emitted before direct method entries.
- `resolveDefaultTarget?` records the inherited-default invariant that lowering
  uses the method owner's trait view, not merely the originally requested trait
  view.

`KernFormal.Mono` tracks the current compiler model at these points:

- `MonoKey` mirrors `MonoModuleMetadata::def_mono_map` and
  `Lowerer::mono_cache`: a definition plus its full `Vec<GenericArg>` is the
  emitted-item identity.
- `requestFunction` mirrors `Lowerer::instantiate_function_at`: a cache hit
  returns the existing `MonoId`, while a miss allocates an id, inserts the key
  immediately, and queues one pending instantiation.
- `drainPending` abstracts
  `drain_pending_function_instantiations_cancelable`: finite pending work is
  consumed without changing monomorphization identities.
- `VtableKey` mirrors `Lowerer::vtable_cache`: vtable identity includes data
  pointer type, matched receiver type, and the concrete trait object view,
  including const-generic trait arguments.
- `stableTypeShiftedConst` models the const-specialization instability case
  reported by `const_specialization_instability_hint`.

`KernFormal.Coherence` tracks the current compiler model at these points:

- `headsOverlap` mirrors the freshened impl-head overlap check in
  `overlapping_trait_impl_pair`, including const-generic literals and params.
- `compareSpecificity` and `coherentPair` mirror the coherence rule that
  overlapping impls are accepted only when one head is strictly more specific.
- `hasLocalAnchor` and `orphanLegal` mirror `type_has_local_impl_anchor` and
  `trait_impl_is_orphan_legal`: imported/foreign traits require a local target
  anchor, and builtin wrappers such as pointers, slices, and arrays preserve
  that anchor.

`KernFormal.ImportMetadata` tracks the current compiler model at these points:

- `importSourceImpl` mirrors `emit_package_metadata` plus the loader/collector
  path for kmeta packages: semantic ids are not serialized, so the consuming
  session reparses the copied source snapshot with `is_imported = true`.
- `SourceImplHead` and `ImportedImplHead` record the source-shape invariant
  needed by imported trait impls: target head, trait id, trait const/type args,
  and associated bindings survive reconstruction unchanged.
- `importedTraitLookupKey` mirrors package-qualified trait registration through
  `register_root_module_package` and `trait_def_lookup_key`; imported traits
  must stay foreign to the consumer package for orphan checks.
- `traitRefFromImported` and `monoKeyFromImportedFunction` connect imported
  const trait args to proof/vtable identity and monomorphization identity.
- `metadataPublishesFunction` and `importedDeclarationLinkage` mirror the
  lowering/linkage invariant for imported trait impl methods: producer packages
  must emit linkable method bodies for metadata consumers, and consumers must
  declare those no-body methods with external linkage.

`KernFormal.PatternMatch` tracks the current compiler model at these points:

- `coverageLower`, `specializeMatrix`, `defaultMatrix`, `vectorUseful`, and
  `findUncovered` mirror the constructor-coverage machinery in
  `compiler/kernc_sema/src/checker/expr/control.rs`.  `coverageLower` is
  target-typed, matching the compiler path that uses the target type to lower
  enum payloads and product fields.
- `prodPartial` records struct/destructure patterns whose omitted fields become
  wildcard coverage entries, matching `coverage_lower_pattern`; lowering tests
  only the written fields, which is semantically the same wildcard treatment.
- `completeValuePattern`, `coverageLowerValueProduct`, and
  `lowerValueProductMatches` record the stricter value-pattern rule for struct
  literals: omitted fields are completed from field defaults, not treated as
  wildcards, so coverage and lowering both test the full literal value.
- `coverageLower (.opaqueUser ...) = none` records that user-defined
  `Pattern[T]` value patterns are runtime tests only; they do not contribute to
  ADT/bool exhaustiveness. This applies recursively: if an enum payload or
  product field is matched by an opaque user pattern such as a slice/string
  predicate, the containing constructor row is also outside the exact coverage
  fragment.
- `lowerFirstMatch` mirrors `Lowerer::lower_match_pattern_chain` in
  `compiler/kernc_lower/src/expr/control/pattern.rs`: arms and alternative
  patterns are tested in source order, and the first matching pattern selects
  the body/bindings.
- `bindingShape` and `sameArmBindings` record the binding-shape invariant
  checked by `pattern_bind_shape` and `match_pattern_bind_shape`: all
  alternatives in one arm must expose the same canonical sorted `(name, type,
  mutability)` list, including bind shapes obtained from `Pattern[T]::Bind`.
- `letElseFailureCovered`, `letElseFirstUncovered`, and `letElseArmUseful`
  record the `let ... else` arm-block invariant in
  `ExprChecker::check_let`: the primary let pattern is inserted into the
  coverage matrix first, so failure arms are checked against exactly the
  remaining complement. The examples include nested enum payload gaps,
  unreachable failure arms shadowed by the success pattern, and opaque
  user-pattern conservatism for any future value-pattern let-else extension.

`KernFormal.ScalarRange` tracks the current compiler model at these points:

- `Domain`, `Interval`, and `Coverage` mirror `ScalarCoverageState`,
  `SignedInterval`, and `UnsignedInterval` in
  `compiler/kernc_sema/src/checker/expr/control.rs`.
- `rangeInterval` mirrors `scalar_range_intervals`: `..=` is closed,
  `...` subtracts one from the end point, and the resulting interval is clipped
  to the target type's finite scalar domain.
- `insertIntervalSorted`, `coversAll`, `isFull`, and `firstUncovered` mirror
  interval insertion/merging, shadowed-pattern detection, exhaustiveness, and
  missing-witness selection for bool and integer matches.
- The examples cover u8/u128/i8 boundaries, empty exclusive ranges, fully
  shadowed subranges, adjacent interval merging, and bool-as-scalar coverage.

`KernFormal.AggregateDecay` tracks the current compiler model at these points:

- `namedToAnonEquivalent` mirrors the named-to-anonymous aggregate branch in
  `ExprChecker::is_anonymous_aggregate_equivalent`: the `extern` bit, field
  count, field names, and instantiated field types must agree before value or
  pointer decay is accepted.
- `fieldsEquivalentForLayout` records the layout-sensitive distinction found
  by the model: native anonymous struct types are canonicalized by name, but
  `extern struct` fields keep declaration order because that order is ABI
  visible.
- `rewriteNamedToAnon?` mirrors
  `Lowerer::rewrite_named_struct_init_to_anon` and
  `Lowerer::rewrite_named_struct_value_to_anon`: after sema has accepted the
  shape, lowering looks up source fields by name and emits fields in the target
  anonymous struct's physical order.
- The examples cover reordered native fields, rejected reordered `extern`
  fields, missing fields, `extern` mismatch, const/generic field substitution,
  and pointer decay reusing the same element-shape equivalence.

`KernFormal.ClosureCallback` tracks the current compiler model at these points:

- `ClosureState` mirrors `TypeKind::AnonymousState { closure_node_id,
  captures, params, ret }`: the closure type identity and lowered state struct
  include capture field types as well as callable signature.
- `coerceStateToFnInterface` mirrors closure BNC in
  `ExprChecker::check_state_to_closure_interface`: signatures must match,
  `&mut Fn` requires a mutable borrow source, and capturing closures record a
  pointer origin for later escape rejection.
- `instantiateSig` and `fnItemFromTemplate` mirror
  `ExprChecker::instantiate_fn_def_signature`: function item callback matching
  substitutes type and const generic arguments before comparing array lengths or
  other const-indexed signature pieces.
- `requestAdapter` mirrors `Lowerer::get_or_create_fn_closure_adapter`: adapter
  identity is the normalized fn-like key, including the full `FnDef` generic
  argument list, so `last[4]` and `last[5]` cannot share a thunk.
- The examples cover const-generic callback acceptance/rejection, same symbolic
  const parameters, mutable captured references, hidden environment parameters,
  noncapturing function-pointer decay, `&mut Fn` borrow gating, escape-origin
  marking, and adapter cache hits/misses.

The executable regression layer remains in
`compiler/kernc_cli/tests/soundness/`.
