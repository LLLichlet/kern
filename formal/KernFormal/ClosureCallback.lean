/-!
Formal model for closure environments and callback identity.

The compiler represents a capturing closure as an anonymous state aggregate plus
a generated entry function.  Coercion to `&Fn` checks only the callable
signature, but the lowered state layout and escape checks still depend on the
captured environment.  Function-item callbacks have a separate adapter cache
whose key must include concrete generic and const-generic arguments.
-/

namespace KernFormal.ClosureCallback

abbrev DefId := Nat
abbrev NodeId := Nat
abbrev TypeId := Nat
abbrev ParamId := Nat
abbrev MonoId := Nat

inductive ConstKey where
  | lit : Nat -> ConstKey
  | param : ParamId -> ConstKey
  deriving DecidableEq, Repr, BEq

inductive Ty where
  | atom : TypeId -> Ty
  | param : ParamId -> Ty
  | array : ConstKey -> Ty -> Ty
  | ptr : Bool -> Ty -> Ty
  deriving DecidableEq, Repr, BEq

inductive GenericArg where
  | ty : Ty -> GenericArg
  | const : ConstKey -> GenericArg
  deriving DecidableEq, Repr, BEq

structure Signature where
  params : List Ty
  ret : Ty
  deriving DecidableEq, Repr, BEq

structure ClosureState where
  node : NodeId
  captures : List Ty
  sig : Signature
  deriving DecidableEq, Repr, BEq

structure FunctionTemplate where
  defn : DefId
  params : List ParamId
  sig : Signature
  deriving DecidableEq, Repr, BEq

structure FnItem where
  defn : DefId
  args : List GenericArg
  sig : Signature
  deriving DecidableEq, Repr, BEq

inductive FnLikeKey where
  | fnDef : DefId -> List GenericArg -> FnLikeKey
  | function : Signature -> FnLikeKey
  deriving DecidableEq, Repr, BEq

abbrev AdapterCache := List (FnLikeKey × MonoId)

def lookupParam? : List (ParamId × GenericArg) -> ParamId -> Option GenericArg
  | [], _ => none
  | binding :: rest, param =>
      if binding.1 == param then some binding.2 else lookupParam? rest param

def substConst (subst : List (ParamId × GenericArg)) : ConstKey -> ConstKey
  | .lit value => .lit value
  | .param param =>
      match lookupParam? subst param with
      | some (.const value) => value
      | _ => .param param

partial def substTy (subst : List (ParamId × GenericArg)) : Ty -> Ty
  | .atom id => .atom id
  | .param param =>
      match lookupParam? subst param with
      | some (.ty ty) => ty
      | _ => .param param
  | .array len elem => .array (substConst subst len) (substTy subst elem)
  | .ptr isMut elem => .ptr isMut (substTy subst elem)

def instantiateSig (template : FunctionTemplate) (args : List GenericArg) : Signature :=
  let subst := template.params.zip args
  { params := template.sig.params.map (substTy subst)
    ret := substTy subst template.sig.ret }

/-- Sema's callback check compares the instantiated callable signature exactly. -/
def callbackCoercible (expected : Signature) (actual : Signature) : Bool :=
  expected == actual

def fnItemFromTemplate (template : FunctionTemplate) (args : List GenericArg) : FnItem :=
  { defn := template.defn
    args := args
    sig := instantiateSig template args }

def fnItemKey (item : FnItem) : FnLikeKey :=
  .fnDef item.defn item.args

def lookupAdapter? : AdapterCache -> FnLikeKey -> Option MonoId
  | [], _ => none
  | entry :: rest, key => if entry.1 == key then some entry.2 else lookupAdapter? rest key

/--
Lowering's function-item-to-`&Fn` adapter cache.  The full fn-like key is used,
so `last[4]` and `last[5]` cannot share an adapter when their signatures differ.
-/
def requestAdapter (cache : AdapterCache) (next : MonoId) (key : FnLikeKey) :
    AdapterCache × MonoId × MonoId :=
  match lookupAdapter? cache key with
  | some id => (cache, next, id)
  | none => ((key, next) :: cache, next + 1, next)

/-- Capturing closure states carry their environment in the lowered struct. -/
def loweredStateFields (state : ClosureState) : List Ty :=
  state.captures

/-- Capturing closure entry functions receive a hidden mutable environment ptr. -/
def closureEntrySignature (state : ClosureState) (decayedToFunction : Bool) : Signature :=
  if decayedToFunction then
    state.sig
  else
    { params := [.ptr true (.atom state.node)] ++ state.sig.params
      ret := state.sig.ret }

/-- Only empty-capture closures may decay to a plain function pointer. -/
def canDecayToFunction (state : ClosureState) (expected : Signature) : Bool :=
  state.captures.isEmpty && callbackCoercible expected state.sig

/--
Coercion to `&Fn` accepts matching signatures.  If captures are present, sema
records a stack-closure pointer origin so later escape checks can reject storage
or return of that fat pointer.
-/
def coerceStateToFnInterface
    (expectedMutable : Bool)
    (canBorrowMutably : Bool)
    (expected : Signature)
    (state : ClosureState) : Option Bool :=
  if expectedMutable && !canBorrowMutably then
    none
  else if callbackCoercible expected state.sig then
    some (!state.captures.isEmpty)
  else
    none

def i32 : Ty := .atom 32
def u8 : Ty := .atom 8
def nConst : ConstKey := .param 1
def arr (len : Nat) : Ty := .array (.lit len) i32
def arrN : Ty := .array nConst i32

def sigArray4 : Signature := { params := [arr 4], ret := i32 }
def sigArray5 : Signature := { params := [arr 5], ret := i32 }
def sigArrayN : Signature := { params := [arrN], ret := i32 }

def lastTemplate : FunctionTemplate :=
  { defn := 10
    params := [1]
    sig := sigArrayN }

def last4 : FnItem := fnItemFromTemplate lastTemplate [.const (.lit 4)]
def last5 : FnItem := fnItemFromTemplate lastTemplate [.const (.lit 5)]
def lastN : FnItem := fnItemFromTemplate lastTemplate [.const (.param 1)]

def capturedRefState : ClosureState :=
  { node := 77
    captures := [.ptr true i32]
    sig := { params := [], ret := i32 } }

def emptyState : ClosureState :=
  { capturedRefState with node := 78, captures := [] }

/-- Function-item callback coercion substitutes const args before comparison. -/
example : callbackCoercible sigArray4 last4.sig = true := by
  native_decide

/-- Mismatched const-instantiated callback signatures are rejected. -/
example : callbackCoercible sigArray4 last5.sig = false := by
  native_decide

/-- Open const parameters match only when both sides keep the same symbolic key. -/
example : callbackCoercible sigArrayN lastN.sig = true := by
  native_decide

/-- Adapter identity includes const-generic function-item arguments. -/
example : (fnItemKey last4 == fnItemKey last5) = false := by
  native_decide

/-- Captures become lowered state fields; mutation through a captured pointer is preserved. -/
example : loweredStateFields capturedRefState = [.ptr true i32] := by
  native_decide

/-- Capturing closure entries receive the hidden environment pointer. -/
example :
    closureEntrySignature capturedRefState false
      = { params := [.ptr true (.atom 77)], ret := i32 } := by
  native_decide

/-- Capturing closures do not decay to plain function pointers. -/
example :
    canDecayToFunction capturedRefState { params := [], ret := i32 } = false := by
  native_decide

/-- Empty closures may decay to plain function pointers when signatures match. -/
example :
    canDecayToFunction emptyState { params := [], ret := i32 } = true := by
  native_decide

/-- `&mut Fn` coercion requires a mutable borrow source. -/
example :
    coerceStateToFnInterface true false { params := [], ret := i32 } capturedRefState = none := by
  native_decide

/-- Capturing `&Fn` coercion records that the resulting pointer can escape. -/
example :
    coerceStateToFnInterface false false { params := [], ret := i32 } capturedRefState
      = some true := by
  native_decide

/-- Noncapturing `&Fn` coercion has no captured environment escape origin. -/
example :
    coerceStateToFnInterface false false { params := [], ret := i32 } emptyState
      = some false := by
  native_decide

/-- Adapter cache hits reuse the existing id instead of creating a new thunk. -/
example :
    requestAdapter [(fnItemKey last4, 9)] 10 (fnItemKey last4)
      = ([(fnItemKey last4, 9)], 10, 9) := by
  native_decide

/-- Adapter cache misses allocate a distinct thunk for a distinct const arg key. -/
example :
    requestAdapter [(fnItemKey last4, 9)] 10 (fnItemKey last5)
      = ([(fnItemKey last5, 10), (fnItemKey last4, 9)], 11, 10) := by
  native_decide

end KernFormal.ClosureCallback
