import KernFormal.FakeRefl

/-!
Formal model for generic associated-type substitution.

The compiler stores one associated-type family target under the trait
associated-type `DefId`.  A projection such as `T.Mapper.Apply[i32]` first
selects that family target from a trait object, environment bound, or impl, and
then applies `Apply`'s own generic arguments to the selected target.  The assoc
arguments are substitutions; they are not a second chance to select a different
impl or reverse-solve const generics.

This file mirrors:

* `SemaContext::instantiate_assoc_projection_target`;
* `substitute_associated_types` for `TypeKind::Associated(def_id, args)`;
* direct projection-argument matching in function-call generic inference.
-/

namespace KernFormal.AssocSubst

open KernFormal.FakeRefl

abbrev ParamId := Nat

/-- A small type language with associated placeholders.

`assocPlaceholder id args` corresponds to `TypeKind::Associated(id, args)` in
the compiler's type registry.  It represents the associated family mention
inside a stored trait/default/supertrait contract before a projection applies
concrete assoc args. -/
inductive AssocTy where
  | atom : Nat -> AssocTy
  | param : ParamId -> AssocTy
  | ptr : AssocTy -> AssocTy
  | app : Nat -> List GenericArg -> AssocTy
  | assocPlaceholder : AssocId -> List GenericArg -> AssocTy
  deriving DecidableEq, Repr

abbrev AssocParamDecls := List ParamId
abbrev AssocFamilyEnv := AssocId -> Option AssocParamDecls
abbrev AssocTargetEnv := AssocId -> Option AssocTy
abbrev AssocSubstMap := List (ParamId × GenericArg)

def lookupParam? : AssocSubstMap -> ParamId -> Option GenericArg
  | [], _ => none
  | binding :: rest, param =>
      if binding.1 = param then some binding.2 else lookupParam? rest param

/-- Substitute a generic argument by the assoc parameter map.

Type arguments may contain assoc type parameters as ordinary `Ty.param` nodes.
Const arguments are substituted only when the whole const generic is a named
parameter; expression folding is intentionally outside this small model. -/
def substGenericArg (subst : AssocSubstMap) : GenericArg -> GenericArg
  | .tyAtom id => .tyAtom id
  | .tyParam param =>
      match lookupParam? subst param with
      | some arg => arg
      | none => .tyParam param
  | .const (.param param) =>
      match lookupParam? subst param with
      | some (.const value) => .const value
      | _ => .const (.param param)
  | .const (.lit value) => .const (.lit value)
  | .const (.add left right) =>
      .const (.add
        (match substGenericArg subst (.const left) with
         | .const value => value
         | _ => left)
        (match substGenericArg subst (.const right) with
         | .const value => value
         | _ => right))

def substAssocTy (subst : AssocSubstMap) (ty : AssocTy) : AssocTy :=
  match ty with
  | .atom id => .atom id
  | .param param =>
      match lookupParam? subst param with
      | some (.tyAtom id) => .atom id
      | some (.tyParam next) => .param next
      | _ => .param param
  | .ptr elem => .ptr (substAssocTy subst elem)
  | .app defId args => .app defId (args.map (substGenericArg subst))
  | .assocPlaceholder assoc args =>
      .assocPlaceholder assoc (args.map (substGenericArg subst))

def buildAssocSubst? (params : AssocParamDecls) (args : List GenericArg) :
    Option AssocSubstMap :=
  if params.length == args.length then
    some (params.zip args)
  else
    none

/-- Apply projection assoc args to the already selected assoc family target.

This is the Lean counterpart of `instantiate_assoc_projection_target`: arity
must match the associated-type declaration, and the result is just substitution
into `selectedTarget`. -/
def instantiateAssocTarget?
    (family : AssocFamilyEnv)
    (assoc : AssocId)
    (assocArgs : List GenericArg)
    (selectedTarget : AssocTy) : Option AssocTy :=
  if assocArgs.isEmpty then
    some selectedTarget
  else
    match family assoc with
    | none => some selectedTarget
    | some params =>
        match buildAssocSubst? params assocArgs with
        | some subst => some (substAssocTy subst selectedTarget)
        | none => none

/-- Substitute explicit associated placeholders using carried assoc bindings.

This abstracts the `TypeKind::Associated` arm of `substitute_associated_types`:
when `Assoc[U]` has a binding to a family target, instantiate the target with
`U`; if there is no binding, keep the placeholder. -/
def substituteAssocPlaceholders?
    (family : AssocFamilyEnv)
    (targets : AssocTargetEnv)
    (ty : AssocTy) : Option AssocTy :=
  match ty with
  | .atom id => some (.atom id)
  | .param param => some (.param param)
  | .ptr elem => Option.map AssocTy.ptr (substituteAssocPlaceholders? family targets elem)
  | .app defId args => some (.app defId args)
  | .assocPlaceholder assoc args =>
      match targets assoc with
      | none => some (.assocPlaceholder assoc args)
      | some selectedTarget => instantiateAssocTarget? family assoc args selectedTarget

def directConstArgMatch? (generic concrete : ConstArg) : Bool :=
  match generic with
  | .param _ => true
  | .lit _ => generic == concrete
  | .add _ _ =>
      if generic == concrete then true else !generic.containsUnresolved && generic == concrete

/-- Direct generic-arg matching used by call inference for projection args.

The generic side may bind a parameter.  A symbolic const expression such as
`N + 1` is not inverted to discover `N`; it matches only if already equal after
substitution/folding. -/
def directGenericArgMatch? (generic concrete : GenericArg) : Bool :=
  match generic, concrete with
  | .tyParam _, .tyAtom _ => true
  | .tyParam left, .tyParam right => left == right
  | .tyAtom left, .tyAtom right => left == right
  | .const left, .const right => directConstArgMatch? left right
  | _, _ => false

def projectionArgsDirectMatch? (generic concrete : List GenericArg) : Bool :=
  generic.length == concrete.length
    && (generic.zip concrete).all (fun pair => directGenericArgMatch? pair.1 pair.2)

theorem empty_assoc_args_keep_selected_target
    (family : AssocFamilyEnv)
    (assoc : AssocId)
    (selectedTarget : AssocTy) :
    instantiateAssocTarget? family assoc [] selectedTarget = some selectedTarget := by
  simp [instantiateAssocTarget?]

theorem nonempty_assoc_arg_arity_mismatch_is_rejected
    (family : AssocFamilyEnv)
    (assoc : AssocId)
    (params : AssocParamDecls)
    (head : GenericArg)
    (tail : List GenericArg)
    (selectedTarget : AssocTy)
    (hFamily : family assoc = some params)
    (hLen : params.length ≠ (head :: tail).length) :
    instantiateAssocTarget? family assoc (head :: tail) selectedTarget = none := by
  have hLen' : params.length ≠ tail.length + 1 := by
    simpa using hLen
  simp [instantiateAssocTarget?, hFamily, buildAssocSubst?, hLen']

def assocApply : AssocId := 10
def paramA : ParamId := 1
def paramN : ParamId := 2
def mapperFamily : AssocFamilyEnv :=
  fun assoc => if assoc = assocApply then some [paramA] else none
def constMapperFamily : AssocFamilyEnv :=
  fun assoc => if assoc = assocApply then some [paramN] else none

def idFamilyTarget : AssocTy := .param paramA
def ptrFamilyTarget : AssocTy := .ptr (.param paramA)
def constArrayFamilyTarget : AssocTy := .app 700 [.const (.param paramN)]

/-- `Id.Mapper.Apply[i32]` normalizes by substituting `i32` into the selected family. -/
example :
    instantiateAssocTarget? mapperFamily assocApply [.tyAtom 32] idFamilyTarget
      = some (.atom 32) := by
  native_decide

/-- The same substitution recurses through containers in the assoc family target. -/
example :
    instantiateAssocTarget? mapperFamily assocApply [.tyAtom 32] ptrFamilyTarget
      = some (.ptr (.atom 32)) := by
  native_decide

/-- Const assoc args are substitutions on the selected family target. -/
example :
    instantiateAssocTarget? constMapperFamily assocApply [.const (.lit 4)]
      constArrayFamilyTarget
      = some (.app 700 [.const (.lit 4)]) := by
  native_decide

/-- Assoc args do not reselect a more convenient target from another impl. -/
example :
    instantiateAssocTarget? mapperFamily assocApply [.tyAtom 32] (.atom 64)
      = some (.atom 64) := by
  native_decide

/-- A bound associated placeholder with args instantiates the stored family target. -/
example :
    substituteAssocPlaceholders?
      mapperFamily
      (fun assoc => if assoc = assocApply then some ptrFamilyTarget else none)
      (.assocPlaceholder assocApply [.tyAtom 32])
      = some (.ptr (.atom 32)) := by
  native_decide

/-- Missing assoc binding leaves the associated placeholder open. -/
example :
    substituteAssocPlaceholders?
      mapperFamily
      (fun _ => none)
      (.assocPlaceholder assocApply [.tyAtom 32])
      = some (.assocPlaceholder assocApply [.tyAtom 32]) := by
  native_decide

/-- Wrong assoc-arg arity is an error, matching the compiler debug/error path. -/
example :
    instantiateAssocTarget? mapperFamily assocApply [.tyAtom 32, .tyAtom 64] idFamilyTarget
      = none := by
  native_decide

/-- Direct projection arg matching can bind a plain const parameter. -/
example :
    projectionArgsDirectMatch? [.const (.param paramN)] [.const (.lit 4)] = true := by
  native_decide

/-- But it does not invert `N + 1` to solve for `N`. -/
example :
    projectionArgsDirectMatch?
      [.const (.add (.param paramN) (.lit 1))]
      [.const (.lit 5)] = false := by
  native_decide

/-- Exact symbolic const expressions may match after earlier substitution. -/
example :
    projectionArgsDirectMatch?
      [.const (.add (.param paramN) (.lit 1))]
      [.const (.add (.param paramN) (.lit 1))] = true := by
  native_decide

end KernFormal.AssocSubst
