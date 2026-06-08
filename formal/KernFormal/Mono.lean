import KernFormal.FakeRefl
import KernFormal.DefaultVtable

/-!
Formal model for monomorphization identity and finite worklist behavior.

This mirrors the lowering-side caches in `compiler/kernc_lower/src/mono.rs` and
`compiler/kernc_lower/src/vtable.rs`:

* function/data identities are keyed by `(DefId, Vec<GenericArg>)`;
* type and const generic arguments are both part of the key;
* a function cache miss allocates a `MonoId`, inserts the key immediately, and
  queues exactly one pending instantiation;
* later requests for the same key return the cached id without queuing another
  job;
* vtables are keyed by normalized data pointer, impl receiver, and target trait
  view, so default-method vtables do not collapse across different trait args.
-/

namespace KernFormal.Mono

open KernFormal.FakeRefl
open KernFormal.DefaultVtable

abbrev DefId := Nat
abbrev MonoId := Nat

structure MonoKey where
  defn : DefId
  args : List GenericArg
  deriving DecidableEq, Repr

instance : BEq MonoKey where
  beq left right := left.defn == right.defn && left.args == right.args

structure VtableKey where
  dataPtr : Ty
  receiver : Ty
  traitRef : TraitRef
  deriving DecidableEq, Repr

instance : BEq VtableKey where
  beq left right :=
    left.dataPtr == right.dataPtr
      && left.receiver == right.receiver
      && left.traitRef == right.traitRef

abbrev MonoCache := List (MonoKey × MonoId)
abbrev VtableCache := List (VtableKey × MonoId)

structure PendingInstantiation where
  key : MonoKey
  id : MonoId
  deriving DecidableEq, Repr

structure MonoState where
  next : MonoId
  cache : MonoCache
  pending : List PendingInstantiation
  deriving DecidableEq, Repr

def lookupMono? : MonoCache -> MonoKey -> Option MonoId
  | [], _ => none
  | entry :: rest, key => if entry.1 = key then some entry.2 else lookupMono? rest key

def lookupVtable? : VtableCache -> VtableKey -> Option MonoId
  | [], _ => none
  | entry :: rest, key => if entry.1 = key then some entry.2 else lookupVtable? rest key

def insertMono (cache : MonoCache) (key : MonoKey) (id : MonoId) : MonoCache :=
  (key, id) :: cache

/-- One request to instantiate a function.

This follows `instantiate_function_at`: cache hit returns immediately; cache
miss allocates a fresh id, inserts the key before lowering the body, and queues
one pending job. -/
def requestFunction (state : MonoState) (key : MonoKey) : MonoState × MonoId :=
  match lookupMono? state.cache key with
  | some id => (state, id)
  | none =>
      let id := state.next
      ({ next := state.next + 1
         cache := insertMono state.cache key id
         pending := state.pending ++ [{ key, id }] }, id)

/-- Drain the finite pending queue.

The compiler lowers bodies while draining; this model keeps only the identity
effect: already queued unique keys are consumed without changing the cache. -/
def drainPending (state : MonoState) : MonoState :=
  { state with pending := [] }

def requestMany (state : MonoState) : List MonoKey -> MonoState × List MonoId
  | [] => (state, [])
  | key :: rest =>
      let (nextState, id) := requestFunction state key
      let (finalState, ids) := requestMany nextState rest
      (finalState, id :: ids)

def typeArgI32 : GenericArg := .tyAtom 32
def typeArgBool : GenericArg := .tyAtom 1
def constArg4 : GenericArg := .const (.lit 4)
def constArg5 : GenericArg := .const (.lit 5)

def keyIdI32 : MonoKey := { defn := 1, args := [typeArgI32] }
def keyIdBool : MonoKey := { defn := 1, args := [typeArgBool] }
def keySlot4 : MonoKey := { defn := 2, args := [constArg4] }
def keySlot5 : MonoKey := { defn := 2, args := [constArg5] }
def keyDefaultSlot4 : MonoKey := { defn := 503, args := [constArg4, .tyAtom 50] }
def keyDefaultSlot5 : MonoKey := { defn := 503, args := [constArg5, .tyAtom 50] }

def emptyState : MonoState := { next := 1, cache := [], pending := [] }

theorem repeated_request_returns_cached_id_without_extra_pending
    (state : MonoState)
    (key : MonoKey)
    (id : MonoId)
    (h : lookupMono? state.cache key = some id) :
    requestFunction state key = (state, id) := by
  simp [requestFunction, h]

theorem miss_inserts_key_before_pending
    (state : MonoState)
    (key : MonoKey)
    (h : lookupMono? state.cache key = none) :
    lookupMono? (requestFunction state key).1.cache key = some state.next := by
  simp [requestFunction, h, lookupMono?, insertMono]

theorem drain_clears_pending
    (state : MonoState) :
    (drainPending state).pending = [] := by
  simp [drainPending]

/-- Type args are part of monomorphization identity. -/
example : (keyIdI32 == keyIdBool) = false := by
  native_decide

/-- Const args are also part of monomorphization identity. -/
example : (keySlot4 == keySlot5) = false := by
  native_decide

/-- Default method instantiations preserve const trait args in their mono key. -/
example : (keyDefaultSlot4 == keyDefaultSlot5) = false := by
  native_decide

/-- A cache miss queues exactly one pending instantiation for that key. -/
example :
    requestFunction emptyState keyIdI32
      = ({ next := 2
           cache := [(keyIdI32, 1)]
           pending := [{ key := keyIdI32, id := 1 }] }, 1) := by
  native_decide

/-- Repeating the same request reuses the existing id and does not grow pending. -/
example :
    let first := (requestFunction emptyState keyIdI32).1
    requestFunction first keyIdI32
      = (first, 1) := by
  native_decide

/-- Large non-recursive batches are finite queues of distinct keys. -/
example :
    let keys := [keySlot4, keySlot5, keyIdI32, keyIdBool]
    (requestMany emptyState keys).1.pending.length = keys.length := by
  native_decide

/-- Draining a finite specialization batch leaves no outstanding jobs. -/
example :
    let state := (requestMany emptyState [keySlot4, keySlot5, keyIdI32]).1
    (drainPending state).pending = [] := by
  native_decide

def traitSlot4 : TraitRef :=
  { trait := traitSlot
    args := [constArg4]
    assocBindings := [] }

def traitSlot5 : TraitRef :=
  { trait := traitSlot
    args := [constArg5]
    assocBindings := [] }

def vtableSlot4 : VtableKey :=
  { dataPtr := .ptr (.atom 50)
    receiver := .atom 50
    traitRef := traitSlot4 }

def vtableSlot5 : VtableKey :=
  { dataPtr := .ptr (.atom 50)
    receiver := .atom 50
    traitRef := traitSlot5 }

/-- Vtable identity includes const-generic trait args. -/
example : (vtableSlot4 == vtableSlot5) = false := by
  native_decide

def stableTypeShiftedConst (previous next : List GenericArg) : Bool :=
  previous.length == next.length
    && (previous.zip next).all (fun pair =>
      match pair.1, pair.2 with
      | .tyAtom left, .tyAtom right => left == right
      | .const _, .const _ => true
      | _, _ => false)
    && (previous.zip next).any (fun pair =>
      match pair.1, pair.2 with
      | .const left, .const right => left != right
      | _, _ => false)

/-- Const-recursive specialization instability is observable even without type growth. -/
example :
    stableTypeShiftedConst [typeArgI32, .const (.lit 0)] [typeArgI32, .const (.lit 1)]
      = true := by
  native_decide

/-- A repeated specialization is stable and should hit the cache instead of growing. -/
example :
    stableTypeShiftedConst [typeArgI32, .const (.lit 0)] [typeArgI32, .const (.lit 0)]
      = false := by
  native_decide

end KernFormal.Mono
