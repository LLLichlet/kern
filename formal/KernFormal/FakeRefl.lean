/-!
Formal model for the Kern fake-reflection / projection soundness boundary.

This is intentionally a small executable specification, not a full compiler
model. It captures the rules that must stay shared by direct proof search,
associated-type projection normalization, trait-object construction, and
supertrait upcast:

* global impl projection is allowed only for fully concrete obligations;
* all query paths use the same specificity frontier;
* a shadowed blanket impl cannot provide an associated-type equality;
* bare trait objects do not infer missing associated bindings;
* supertrait upcasts preserve inherited associated bindings;
* const-generic and type-generic variables keep projections open.
-/

namespace KernFormal.FakeRefl

abbrev TraitId := Nat
abbrev AssocId := Nat
abbrev ImplId := Nat

/-- Generic arguments are split only as far as the fake-reflection invariant needs.

`tyAtom` stands for an already-substituted concrete type argument, while
`tyParam` stands for an unresolved type parameter. `const value` carries the
small const-generic expression language above. This mirrors
the checks in `projection_is_fully_concrete` and
`projection_assoc_from_global_impls`, without trying to model every `TypeKind`
container here. -/
inductive ConstArg where
  | lit : Nat -> ConstArg
  | param : Nat -> ConstArg
  | add : ConstArg -> ConstArg -> ConstArg
  deriving DecidableEq, Repr

/-- Whether a const generic still contains a symbolic parameter.

This corresponds to `TypeRegistry::const_generic_contains_params`, including
recursive `ConstGeneric::Expr` forms such as `N + 1`. -/
def ConstArg.containsUnresolved : ConstArg -> Bool
  | .lit _ => false
  | .param _ => true
  | .add left right => left.containsUnresolved || right.containsUnresolved

/-- Occurs check for const-generic unification.

This models `const_param_occurs_in_const_generic_with_map`: `N` may be bound to
`4`, but not to an expression containing `N`, such as `N + 1`. -/
def ConstArg.occurs (needle : Nat) : ConstArg -> Bool
  | .lit _ => false
  | .param name => name == needle
  | .add left right => left.occurs needle || right.occurs needle

inductive GenericArg where
  | tyAtom : Nat -> GenericArg
  | tyParam : Nat -> GenericArg
  | const : ConstArg -> GenericArg
  deriving DecidableEq, Repr

def GenericArg.containsUnresolved : GenericArg -> Bool
  | .tyAtom _ => false
  | .tyParam _ => true
  | .const value => value.containsUnresolved

/-- A small type language for projection targets and associated-type results.

`app def args` corresponds to compiler container forms that carry generic
arguments, such as `TypeKind::Def(_, Vec<GenericArg>)`. The model deliberately
keeps generic arguments as the lightweight `GenericArg` above so the proof stays
focused on the solver boundary rather than on every recursive Rust `TypeKind`
case. -/
inductive Ty where
  | atom : Nat -> Ty
  | param : Nat -> Ty
  | ptr : Ty -> Ty
  | app : Nat -> List GenericArg -> Ty
  deriving DecidableEq, Repr

/-- Trait-object associated bindings are stored by associated-type definition id.

This mirrors the compiler's `Vec<(DefId, TypeId)>` in
`TypeKind::TraitObject`. -/
abbrev AssocBinding := AssocId × Ty

/-- A trait-object head plus the assoc equalities currently visible on it.

This corresponds to `TypeKind::TraitObject(def_id, args, assoc_bindings)`.
For a bare trait object, `assocBindings = []`; that absence is significant and
must not be filled from global impl selection. -/
structure TraitRef where
  trait : TraitId
  args : List GenericArg
  assocBindings : List AssocBinding
  deriving DecidableEq, Repr

/-- A trait proof obligation, i.e. "target implements traitRef". -/
structure Obligation where
  target : Ty
  traitRef : TraitRef
  deriving DecidableEq, Repr

/-- Associated projection `target.Trait[args].Assoc[assocArgs]`.

This corresponds to `TypeKind::Projection`. The concrete gate must inspect the
target, trait args, visible assoc bindings, and assoc generic args before any
global impl projection is allowed. -/
structure Projection where
  obligation : Obligation
  assoc : AssocId
  assocArgs : List GenericArg
  deriving DecidableEq, Repr

/-- Conservative open-term detector used by the global-impl projection gate. -/
def Ty.containsUnresolved : Ty -> Bool
  | .atom _ => false
  | .param _ => true
  | .ptr elem => Ty.containsUnresolved elem
  | .app _ args => args.any GenericArg.containsUnresolved

def Ty.isConcrete (ty : Ty) : Bool :=
  !ty.containsUnresolved

def GenericArg.isConcrete (arg : GenericArg) : Bool :=
  !arg.containsUnresolved

/-- A tiny direct const-generic matching rule.

This abstracts `match_available_const_generic_against_requirement`: a candidate
impl-side const param can bind to a concrete requirement, but the occurs check
rejects recursive bindings such as `N := N + 1`. If the requirement side is the
open parameter, matching is exact only; that is the "no reverse solving" rule. -/
def matchConstArg? (available required : ConstArg) : Option (Nat × ConstArg) :=
  match available, required with
  | .param name, other =>
      if other.occurs name then none else some (name, other)
  | other, .param _ =>
      if other == required then some (0, required) else none
  | other, exact =>
      if other == exact then some (0, exact) else none

def TraitRef.isConcrete (traitRef : TraitRef) : Bool :=
  traitRef.args.all GenericArg.isConcrete
    && traitRef.assocBindings.all (fun binding => Ty.isConcrete binding.2)

def Obligation.isConcrete (obligation : Obligation) : Bool :=
  obligation.target.isConcrete && obligation.traitRef.isConcrete

def Projection.isConcrete (projection : Projection) : Bool :=
  projection.obligation.isConcrete && projection.assocArgs.all GenericArg.isConcrete

/-- One applicable impl candidate for a proof/projection query.

`specificity` abstracts `compare_impl_specificity`: larger means strictly more
specific. In the compiler this relation is not a total order; when two
candidates are incomparable they must both survive the frontier and the query
must refuse to invent a result. -/
structure Candidate where
  impl : ImplId
  result : Ty
  specificity : Nat
  deriving DecidableEq, Repr

instance : BEq Candidate where
  beq left right :=
    left.impl == right.impl && left.result == right.result && left.specificity == right.specificity

def undominatedBy (other candidate : Candidate) : Bool :=
  candidate.specificity < other.specificity

def isMaximalIn (candidates : List Candidate) (candidate : Candidate) : Bool :=
  candidates.any (fun item => item == candidate)
    && !candidates.any (fun other => undominatedBy other candidate)

def maximalCandidates (candidates : List Candidate) : List Candidate :=
  candidates.filter (isMaximalIn candidates)

/-- The sound selection rule shared by direct proof and projection queries.

This models the `collect_specificity_maximal_*` helpers: keep undominated
candidates, then continue only when exactly one remains. -/
def uniqueMaximal? (candidates : List Candidate) : Option Candidate :=
  match maximalCandidates candidates with
  | [candidate] => some candidate
  | _ => none

/-- Normalize a projection from global impls only.

This is intentionally guarded by `Projection.isConcrete`. Open projections may
be carried until a local env bound or later substitution can prove them; they
must not reverse-solve generics through blanket impls. -/
def normalizeProjection? (projection : Projection) (candidates : List Candidate) : Option Ty :=
  if projection.isConcrete then
    (uniqueMaximal? candidates).map (fun candidate => candidate.result)
  else
    none

/-- Direct trait proof uses the same concreteness and specificity frontier. -/
def directProofCandidate? (obligation : Obligation) (candidates : List Candidate) :
    Option Candidate :=
  if obligation.isConcrete then
    uniqueMaximal? candidates
  else
    none

/-- Lookup an explicitly carried assoc binding. No impl search happens here. -/
def assocLookup? : List AssocBinding -> AssocId -> Option Ty
  | [], _ => none
  | binding :: rest, assoc =>
      if binding.1 = assoc then some binding.2 else assocLookup? rest assoc

/-- Project from a trait object by using only bindings already visible on it. -/
def traitObjectProjection? (traitObject : TraitRef) (assoc : AssocId) : Option Ty :=
  assocLookup? traitObject.assocBindings assoc

/-- Project from active where-clause/environment bounds.

This corresponds to `projection_assoc_from_env_bounds`: env facts are local
proof inputs and may resolve open projections without falling back to global
impl selection. -/
def envProjection? (envBounds : List TraitRef) (projection : Projection) : Option Ty :=
  envBounds.findSome? (fun bound =>
    if bound.trait == projection.obligation.traitRef.trait
        && bound.args == projection.obligation.traitRef.args then
      assocLookup? bound.assocBindings projection.assoc
    else
      none)

/-- The source order used by compiler projection normalization.

This mirrors `ExprChecker::try_normalize_projection`:
1. explicit trait-object/hierarchy assoc binding;
2. active env bound;
3. global impl projection, guarded by concreteness and uniqueness.
-/
def normalizeProjectionFromCompilerSources?
    (targetTraitObject : Option TraitRef)
    (envBounds : List TraitRef)
    (projection : Projection)
    (globalCandidates : List Candidate) : Option Ty :=
  match targetTraitObject.bind (fun traitObject => traitObjectProjection? traitObject projection.assoc) with
  | some ty => some ty
  | none =>
      match envProjection? envBounds projection with
      | some ty => some ty
      | none => normalizeProjection? projection globalCandidates

/-- Old replacement-style selection shape.

This is intentionally kept in the model as a regression tripwire. It is sound
only when it agrees with `uniqueMaximal?`; otherwise a scan can pick one
candidate out of an ambiguous set. -/
def selectByReplacement? : List Candidate -> Option Candidate
  | [] => none
  | candidate :: rest =>
      rest.foldl
        (fun selected current =>
          match selected with
          | none => some current
          | some old =>
              if old.specificity < current.specificity then some current else selected)
        (some candidate)

/-- Trait-object bindings are stronger than env/global facts. -/
theorem trait_object_binding_precedes_env_and_global
    (traitObject : TraitRef)
    (envBounds : List TraitRef)
    (projection : Projection)
    (globalCandidates : List Candidate)
    (assocTy : Ty)
    (h : traitObjectProjection? traitObject projection.assoc = some assocTy) :
    normalizeProjectionFromCompilerSources?
        (some traitObject)
        envBounds
        projection
        globalCandidates = some assocTy := by
  simp [normalizeProjectionFromCompilerSources?, h]

/-- Local env bounds are stronger than global impl projection. -/
theorem env_binding_precedes_global
    (envBounds : List TraitRef)
    (projection : Projection)
    (globalCandidates : List Candidate)
    (assocTy : Ty)
    (h : envProjection? envBounds projection = some assocTy) :
    normalizeProjectionFromCompilerSources?
        none
        envBounds
        projection
        globalCandidates = some assocTy := by
  simp [normalizeProjectionFromCompilerSources?, h]

/-- If object and env sources are missing, an open projection still cannot use globals. -/
theorem open_projection_uses_no_global_impls_after_missing_object_and_env
    (projection : Projection)
    (candidates : List Candidate)
    (hConcrete : projection.isConcrete = false)
    (hEnv : envProjection? [] projection = none := by rfl) :
    normalizeProjectionFromCompilerSources? none [] projection candidates = none := by
  simp [normalizeProjectionFromCompilerSources?, normalizeProjection?, hConcrete, hEnv]

def retainAssocBindings (allowed : List AssocId) (bindings : List AssocBinding) :
    List AssocBinding :=
  bindings.filter (fun binding => binding.1 ∈ allowed)

/-- Compare two target trait views after retaining only assoc ids declared by the target trait.

This models `target_trait_views_equivalent`, which first calls
`retain_declared_trait_object_assoc_bindings` and then compares the target views.
Only declared target assoc bindings participate in diamond-path conflict checks. -/
def traitViewsEquivalent (declaredAssocIds : List AssocId) (left right : TraitRef) : Bool :=
  left.trait == right.trait
    && left.args == right.args
    && retainAssocBindings declaredAssocIds left.assocBindings
      == retainAssocBindings declaredAssocIds right.assocBindings

/-- Combine two independently discovered supertrait target views.

The compiler walks every supertrait path. The first matching target view is held
tentatively; later matching views must be equivalent or the walk returns `None`
instead of picking one diamond branch silently. -/
def mergeTraitViews? (declaredAssocIds : List AssocId) (left right : TraitRef) :
    Option TraitRef :=
  if traitViewsEquivalent declaredAssocIds left right then some left else none

/-- Fold the target views found through a trait hierarchy.

This abstracts `trait_object_view_from_hierarchy_inner` after substitution and
assoc-binding augmentation have produced candidate target views. -/
def mergeTraitViewPaths? (declaredAssocIds : List AssocId) : List TraitRef -> Option TraitRef
  | [] => none
  | view :: rest =>
      rest.foldl
        (fun acc next =>
          match acc with
          | none => none
          | some current => mergeTraitViews? declaredAssocIds current next)
        (some view)

/-- Supertrait upcast rewrites the trait head but only retains validated bindings.

The compiler version walks `resolved_supertraits`, substitutes trait arguments,
and retains/inherits assoc bindings along that validated path. -/
def upcastTraitObject
    (source : TraitRef)
    (superTrait : TraitId)
    (superArgs : List GenericArg)
    (superAssocIds : List AssocId) : TraitRef :=
  { trait := superTrait
    args := superArgs
    assocBindings := retainAssocBindings superAssocIds source.assocBindings }

/-- Open projections cannot use global impls even if candidates exist. -/
theorem open_projection_does_not_use_global_impls
    (projection : Projection)
    (candidates : List Candidate)
    (h : projection.isConcrete = false) :
    normalizeProjection? projection candidates = none := by
  simp [normalizeProjection?, h]

theorem open_direct_proof_does_not_use_global_impls
    (obligation : Obligation)
    (candidates : List Candidate)
    (h : obligation.isConcrete = false) :
    directProofCandidate? obligation candidates = none := by
  simp [directProofCandidate?, h]

/-- A bare trait object has no hidden assoc equality. -/
theorem bare_trait_object_does_not_infer_assoc
    (traitId : TraitId)
    (args : List GenericArg)
    (assoc : AssocId) :
    traitObjectProjection? { trait := traitId, args := args, assocBindings := [] } assoc = none := by
  simp [traitObjectProjection?, assocLookup?]

/-- A retained binding remains projectable after an upcast. -/
theorem retained_assoc_binding_is_projectable
    (source : TraitRef)
    (superTrait : TraitId)
    (superArgs : List GenericArg)
    (assoc : AssocId)
    (assocTy : Ty)
    (rest : List AssocBinding)
    (allowed : List AssocId)
    (hAllowed : assoc ∈ allowed) :
    traitObjectProjection?
        (upcastTraitObject
          { source with assocBindings := (assoc, assocTy) :: rest }
          superTrait
          superArgs
          allowed)
        assoc = some assocTy := by
  simp [traitObjectProjection?, upcastTraitObject, retainAssocBindings, assocLookup?, hAllowed]

/-- A binding that is not part of the target supertrait view is dropped. -/
theorem dropped_assoc_binding_is_not_projectable
    (source : TraitRef)
    (superTrait : TraitId)
    (superArgs : List GenericArg)
    (assoc : AssocId)
    (assocTy : Ty)
    (allowed : List AssocId)
    (hDropped : assoc ∉ allowed) :
    traitObjectProjection?
        (upcastTraitObject
          { source with assocBindings := [(assoc, assocTy)] }
          superTrait
          superArgs
          allowed)
        assoc = none := by
  simp [traitObjectProjection?, upcastTraitObject, retainAssocBindings, assocLookup?, hDropped]

theorem conflicting_supertrait_views_are_rejected
    (declaredAssocIds : List AssocId)
    (left right : TraitRef)
    (h : traitViewsEquivalent declaredAssocIds left right = false) :
    mergeTraitViews? declaredAssocIds left right = none := by
  simp [mergeTraitViews?, h]

def traitTypeIs : TraitId := 1
def assocIs : AssocId := 1
def tyI32 : Ty := .atom 32
def tyFnI32 : Ty := .atom 100
def tyFakeProof (_left _right : Ty) : Ty := .app 200 [.tyAtom 100, .tyAtom 32]

def blanketTypeIs (arg : Ty) : Candidate :=
  { impl := 1, result := arg, specificity := 0 }

def fakeProofTypeIs (left : Ty) : Candidate :=
  { impl := 2, result := left, specificity := 1 }

def leftWhereTypeIs : Candidate :=
  { impl := 3, result := tyI32, specificity := 0 }

def rightWhereTypeIs : Candidate :=
  { impl := 4, result := tyFnI32, specificity := 0 }

def fakeReflConcreteProjection : Projection :=
  { obligation :=
      { target := tyFakeProof tyFnI32 tyI32
        traitRef :=
          { trait := traitTypeIs
            args := [.tyAtom 32]
            assocBindings := [] } }
    assoc := assocIs
    assocArgs := [] }

example :
    normalizeProjection?
      fakeReflConcreteProjection
      [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32] = some tyFnI32 := by
  native_decide

example :
    normalizeProjectionFromCompilerSources?
      none
      []
      fakeReflConcreteProjection
      [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32] = some tyFnI32 := by
  native_decide

example :
    normalizeProjectionFromCompilerSources?
      (some { trait := traitTypeIs, args := [.tyAtom 32], assocBindings := [(assocIs, tyI32)] })
      []
      fakeReflConcreteProjection
      [fakeProofTypeIs tyFnI32] = some tyI32 := by
  native_decide

example :
    normalizeProjectionFromCompilerSources?
      none
      [{ trait := traitTypeIs, args := [.tyAtom 32], assocBindings := [(assocIs, tyI32)] }]
      fakeReflConcreteProjection
      [fakeProofTypeIs tyFnI32] = some tyI32 := by
  native_decide

example :
    selectByReplacement? [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32]
      = uniqueMaximal? [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32] := by
  native_decide

example :
    uniqueMaximal? [leftWhereTypeIs, rightWhereTypeIs] = none := by
  native_decide

example :
    normalizeProjection?
      fakeReflConcreteProjection
      [leftWhereTypeIs, rightWhereTypeIs] = none := by
  native_decide

example :
    selectByReplacement? [leftWhereTypeIs, rightWhereTypeIs] = some leftWhereTypeIs := by
  native_decide

example :
    directProofCandidate?
      fakeReflConcreteProjection.obligation
      [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32] = some (fakeProofTypeIs tyFnI32) := by
  native_decide

def openGenericProjection : Projection :=
  { fakeReflConcreteProjection with
    obligation :=
      { fakeReflConcreteProjection.obligation with
        target := .param 0 } }

example :
    normalizeProjection?
      openGenericProjection
      [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32] = none := by
  native_decide

def openConstGenericProjection : Projection :=
  { fakeReflConcreteProjection with
    obligation :=
      { fakeReflConcreteProjection.obligation with
        target := .app 300 [.const (.param 0)] } }

example :
    normalizeProjection?
      openConstGenericProjection
      [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32] = none := by
  native_decide

def assocGenericProjection : Projection :=
  { fakeReflConcreteProjection with
    assocArgs := [.tyParam 9] }

example :
    normalizeProjection?
      assocGenericProjection
      [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32] = none := by
  native_decide

example :
    traitObjectProjection?
      { trait := traitTypeIs, args := [.tyAtom 32], assocBindings := [] }
      assocIs = none := by
  native_decide

example :
    traitObjectProjection?
      (upcastTraitObject
        { trait := 2
          args := [.const (.lit 4)]
          assocBindings := [(assocIs, tyFnI32)] }
        traitTypeIs
        [.const (.lit 4)]
        [assocIs])
      assocIs = some tyFnI32 := by
  native_decide

def leftDiamondBaseView : TraitRef :=
  { trait := traitTypeIs
    args := []
    assocBindings := [(assocIs, tyI32)] }

def rightDiamondBaseViewSame : TraitRef :=
  { trait := traitTypeIs
    args := []
    assocBindings := [(assocIs, tyI32)] }

def rightDiamondBaseViewConflict : TraitRef :=
  { trait := traitTypeIs
    args := []
    assocBindings := [(assocIs, tyFnI32)] }

example :
    mergeTraitViewPaths? [assocIs] [leftDiamondBaseView, rightDiamondBaseViewSame]
      = some leftDiamondBaseView := by
  native_decide

example :
    mergeTraitViewPaths? [assocIs] [leftDiamondBaseView, rightDiamondBaseViewConflict]
      = none := by
  native_decide

example :
    mergeTraitViewPaths? [] [leftDiamondBaseView, rightDiamondBaseViewConflict]
      = some leftDiamondBaseView := by
  native_decide

example :
    matchConstArg? (.param 0) (.lit 4) = some (0, .lit 4) := by
  native_decide

example :
    matchConstArg? (.param 0) (.add (.param 0) (.lit 1)) = none := by
  native_decide

example :
    matchConstArg? (.add (.param 0) (.lit 1)) (.param 0) = none := by
  native_decide

example :
    GenericArg.isConcrete (.const (.add (.param 0) (.lit 1))) = false := by
  native_decide

def openConstExprProjection : Projection :=
  { fakeReflConcreteProjection with
    obligation :=
      { fakeReflConcreteProjection.obligation with
        target := .app 300 [.const (.add (.param 0) (.lit 1))] } }

example :
    normalizeProjection?
      openConstExprProjection
      [blanketTypeIs tyI32, fakeProofTypeIs tyFnI32] = none := by
  native_decide

end KernFormal.FakeRefl
