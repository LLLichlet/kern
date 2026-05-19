import KernFormal.FakeRefl
import KernFormal.MethodLookup

/-!
Formal model for trait default-method lowering and vtable entries.

This mirrors the recently added default-method path in lowering:

* `LoweringContext::build_and_inject_vtable_global` chooses an explicit impl
  method before falling back to a trait default body;
* `trait_default_function_args` and
  `resolve_trait_default_method_target` instantiate default bodies with the
  owner trait's generic arguments plus the matched receiver type as hidden
  `Self`;
* inherited default methods must use the trait view that actually owns the
  method, not the originally requested super/subtrait view.
-/

namespace KernFormal.DefaultVtable

open KernFormal.FakeRefl
open KernFormal.MethodLookup

abbrev VtableSlot := Nat

/-- A default method body plus the trait view that owns it.

`traitArgs` corresponds to the `TypeKind::TraitObject(_, trait_args, _)`
arguments used by lowering when it builds a `FnDef(default_id, args)`. -/
structure DefaultBody where
  method : MethodId
  trait : TraitId
  traitArgs : List GenericArg
  defaultId : MethodId
  deriving DecidableEq, Repr

/-- An explicit impl method that can override a trait default in a vtable slot. -/
structure ImplMethodBody where
  method : MethodId
  impl : ImplId
  implArgs : List GenericArg
  functionId : MethodId
  deriving DecidableEq, Repr

inductive VtableEntry where
  | implMethod : ImplMethodBody -> VtableEntry
  | defaultMethod : DefaultBody -> List GenericArg -> VtableEntry
  | missing : VtableEntry
  deriving DecidableEq, Repr

instance : BEq DefaultBody where
  beq left right :=
    left.method == right.method
      && left.trait == right.trait
      && left.traitArgs == right.traitArgs
      && left.defaultId == right.defaultId

instance : BEq ImplMethodBody where
  beq left right :=
    left.method == right.method
      && left.impl == right.impl
      && left.implArgs == right.implArgs
      && left.functionId == right.functionId

/-- A compact, stable code for using `Ty` as a generic arg in this model.

The compiler stores the actual `TypeId`.  The model only needs to distinguish
receiver shapes, so a structural code is enough for executable examples. -/
def reprTy : Ty -> Nat
  | .atom id => id
  | .param id => 10_000 + id
  | .ptr elem => 20_000 + reprTy elem
  | .app defId args => 30_000 + defId + args.length

/-- Append hidden `Self` to trait default function args.

This is the core invariant behind `trait_default_function_args`: default
function generics are the owning trait arguments followed by the concrete
receiver type.  For const-generic traits this preserves const args such as
`Slot[4]`; for pointer impls it preserves receiver shapes such as `&usize`. -/
def defaultFunctionArgs (ownerTraitArgs : List GenericArg) (receiver : Ty) :
    List GenericArg :=
  ownerTraitArgs ++ [.tyAtom (reprTy receiver)]

def findImplMethod? (method : MethodId) : List ImplMethodBody -> Option ImplMethodBody
  | [] => none
  | body :: rest =>
      if body.method == method then some body else findImplMethod? method rest

def findDefault? (method : MethodId) : List DefaultBody -> Option DefaultBody
  | [] => none
  | body :: rest =>
      if body.method == method then some body else findDefault? method rest

/-- Build one method slot in the same order as vtable lowering.

Explicit impl methods win.  If the impl omits the method, a default body is
instantiated with the owning trait args and matched receiver type. -/
def buildVtableMethodEntry
    (method : MethodId)
    (receiver : Ty)
    (implMethods : List ImplMethodBody)
    (defaults : List DefaultBody) : VtableEntry :=
  match findImplMethod? method implMethods with
  | some implBody => .implMethod implBody
  | none =>
      match findDefault? method defaults with
      | some defaultBody =>
          .defaultMethod defaultBody (defaultFunctionArgs defaultBody.traitArgs receiver)
      | none => .missing

/-- Supertrait vtable layout stores transitive super vtables before direct methods. -/
def buildVtable
    (superSlots : List VtableSlot)
    (methods : List MethodId)
    (receiver : Ty)
    (implMethods : List ImplMethodBody)
    (defaults : List DefaultBody) : List VtableEntry :=
  superSlots.map (fun _ => .missing)
    ++ methods.map (fun method => buildVtableMethodEntry method receiver implMethods defaults)

/-- Static/bound default dispatch uses the owner trait view returned by sema.

This abstracts `resolve_trait_default_method_target`: if method lookup reaches a
default inherited from a parent trait, lowering must instantiate the parent
trait's args, not the child trait's args. -/
def resolveDefaultTarget?
    (owner : DefaultBody)
    (receiver : Ty)
    (requestedTraitArgs : List GenericArg) : Option (MethodId × List GenericArg) :=
  if owner.traitArgs == requestedTraitArgs then
    some (owner.defaultId, defaultFunctionArgs owner.traitArgs receiver)
  else
    some (owner.defaultId, defaultFunctionArgs owner.traitArgs receiver)

theorem explicit_impl_method_precedes_default
    (method : MethodId)
    (receiver : Ty)
    (implBody : ImplMethodBody)
    (defaults : List DefaultBody)
    (h : implBody.method = method) :
    buildVtableMethodEntry method receiver [implBody] defaults = .implMethod implBody := by
  simp [buildVtableMethodEntry, findImplMethod?, h]

theorem missing_impl_uses_default_body
    (method : MethodId)
    (receiver : Ty)
    (defaultBody : DefaultBody)
    (h : defaultBody.method = method) :
    buildVtableMethodEntry method receiver [] [defaultBody]
      = .defaultMethod defaultBody (defaultFunctionArgs defaultBody.traitArgs receiver) := by
  simp [buildVtableMethodEntry, findImplMethod?, findDefault?, h]

def traitScore : TraitId := 100
def traitBoxed : TraitId := 101
def traitSlot : TraitId := 102
def traitRender : TraitId := 103

def methodValue : MethodId := 1
def methodFallback : MethodId := 2

def recvX : Ty := .atom 50
def recvPtrUsize : Ty := .ptr (.atom 15)

def scoreDefault : DefaultBody :=
  { method := methodValue
    trait := traitScore
    traitArgs := []
    defaultId := 501 }

def scoreOverride : ImplMethodBody :=
  { method := methodValue
    impl := 1
    implArgs := []
    functionId := 601 }

def boxedDefault : DefaultBody :=
  { method := methodValue
    trait := traitBoxed
    traitArgs := [.tyAtom 32]
    defaultId := 502 }

def slotConstDefault : DefaultBody :=
  { method := methodValue
    trait := traitSlot
    traitArgs := [.const (.lit 4)]
    defaultId := 503 }

def renderDefault : DefaultBody :=
  { method := methodFallback
    trait := traitRender
    traitArgs := []
    defaultId := 504 }

/-- Trait object vtables use default bodies for omitted impl methods. -/
example :
    buildVtableMethodEntry methodValue recvX [] [scoreDefault]
      = .defaultMethod scoreDefault [.tyAtom (reprTy recvX)] := by
  native_decide

/-- Trait object vtables use explicit impl methods instead of default bodies. -/
example :
    buildVtableMethodEntry methodValue recvX [scoreOverride] [scoreDefault]
      = .implMethod scoreOverride := by
  native_decide

/-- Default method dispatch preserves trait type arguments. -/
example :
    buildVtableMethodEntry methodValue recvX [] [boxedDefault]
      = .defaultMethod boxedDefault [.tyAtom 32, .tyAtom (reprTy recvX)] := by
  native_decide

/-- Default method dispatch preserves const-generic trait arguments. -/
example :
    buildVtableMethodEntry methodValue recvX [] [slotConstDefault]
      = .defaultMethod slotConstDefault [.const (.lit 4), .tyAtom (reprTy recvX)] := by
  native_decide

/-- Default method lowering keeps pointer-shaped `Self` for pointer impls. -/
example :
    buildVtableMethodEntry methodFallback recvPtrUsize [] [renderDefault]
      = .defaultMethod renderDefault [.tyAtom (reprTy recvPtrUsize)] := by
  native_decide

/-- Supertrait slots come before direct method slots in the vtable layout. -/
example :
    buildVtable [0, 1] [methodValue] recvX [] [scoreDefault]
      = [.missing, .missing,
         .defaultMethod scoreDefault [.tyAtom (reprTy recvX)]] := by
  native_decide

/-- Inherited default dispatch uses the owner trait args rather than requested args. -/
example :
    resolveDefaultTarget?
      boxedDefault
      recvX
      [.tyAtom 64] = some (boxedDefault.defaultId, [.tyAtom 32, .tyAtom (reprTy recvX)]) := by
  native_decide

end KernFormal.DefaultVtable
