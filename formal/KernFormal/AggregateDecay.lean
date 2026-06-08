/-!
Formal model for implicit aggregate decay.

Kern allows named structs/unions and anonymous aggregates to interoperate when
their ABI-relevant shapes agree.  The soundness-critical part for structs is
that semantic checking compares the same field-name/type set that lowering uses
to reorder values into the target physical layout.

This model focuses on struct value/pointer decay:

* named generic fields are instantiated before comparison;
* `extern` layout markers must agree;
* native fields may appear in different source/physical orders, but names and
  instantiated types must match exactly;
* `extern struct` fields preserve declaration order because it is ABI-visible;
* lowering rewrites by target field order only after sema equivalence holds.
-/

namespace KernFormal.AggregateDecay

abbrev FieldId := Nat
abbrev TypeId := Nat
abbrev ParamId := Nat

inductive Ty where
  | atom : TypeId -> Ty
  | param : ParamId -> Ty
  | array : Nat -> Ty -> Ty
  | ptr : Bool -> Ty -> Ty
  deriving Repr, BEq

structure Field where
  name : FieldId
  ty : Ty
  deriving Repr, BEq

structure NamedStruct where
  isExtern : Bool
  params : List ParamId
  fields : List Field
  physicalToAst : List Nat
  deriving Repr, BEq

structure AnonStruct where
  isExtern : Bool
  fields : List Field
  physicalToAst : List Nat
  deriving Repr, BEq

structure ValueExpr where
  field : FieldId
  ty : Ty
  deriving Repr, BEq

def listGet? : List α -> Nat -> Option α
  | [], _ => none
  | value :: _, 0 => some value
  | _ :: rest, n + 1 => listGet? rest n

def lookupParam? : List (ParamId × Ty) -> ParamId -> Option Ty
  | [], _ => none
  | binding :: rest, param =>
      if binding.1 == param then some binding.2 else lookupParam? rest param

partial def substTy (subst : List (ParamId × Ty)) : Ty -> Ty
  | .atom id => .atom id
  | .param param =>
      match lookupParam? subst param with
      | some ty => ty
      | none => .param param
  | .array len elem => .array len (substTy subst elem)
  | .ptr isMut elem => .ptr isMut (substTy subst elem)

def instantiateFields
    (params : List ParamId)
    (args : List Ty)
    (fields : List Field) : List Field :=
  let subst := params.zip args
  fields.map (fun field => { field with ty := substTy subst field.ty })

partial def insertFieldByName (field : Field) : List Field -> List Field
  | [] => [field]
  | head :: rest =>
      if field.name <= head.name then field :: head :: rest
      else head :: insertFieldByName field rest

def canonicalFields (fields : List Field) : List Field :=
  fields.foldl (fun sorted field => insertFieldByName field sorted) []

def fieldsEquivalent (left right : List Field) : Bool :=
  canonicalFields left == canonicalFields right

/--
Native structs have structural field identity, so field declaration order is not
part of equivalence. `extern struct` is ABI-facing and must preserve the source
field order used by layout.
-/
def fieldsEquivalentForLayout (isExtern : Bool) (left right : List Field) : Bool :=
  if isExtern then left == right else fieldsEquivalent left right

/-- Sema's named-to-anonymous struct equivalence check. -/
def namedToAnonEquivalent (named : NamedStruct) (args : List Ty) (anon : AnonStruct) : Bool :=
  named.isExtern == anon.isExtern
    && named.fields.length == anon.fields.length
    && fieldsEquivalentForLayout
      named.isExtern
      (instantiateFields named.params args named.fields)
      anon.fields

def zipWithIndexFrom : List α -> Nat -> List (α × Nat)
  | [], _ => []
  | head :: rest, index => (head, index) :: zipWithIndexFrom rest (index + 1)

def zipWithIndex (values : List α) : List (α × Nat) :=
  zipWithIndexFrom values 0

def sourceByNameFromPhysical
    (fields : List Field)
    (physicalToAst : List Nat)
    (values : List ValueExpr) : List (FieldId × ValueExpr) :=
  (zipWithIndex physicalToAst).filterMap (fun pair =>
    let astIndex := pair.1
    let physicalIndex := pair.2
    match listGet? fields astIndex, listGet? values physicalIndex with
    | some field, some value => some (field.name, value)
    | _, _ => none)

def lookupValue? : List (FieldId × ValueExpr) -> FieldId -> Option ValueExpr
  | [], _ => none
  | binding :: rest, name =>
      if binding.1 == name then some binding.2 else lookupValue? rest name

/-- Lowering's field rewrite into the anonymous target's physical order. -/
def rewriteNamedToAnon?
    (named : NamedStruct)
    (args : List Ty)
    (anon : AnonStruct)
    (valuesInNamedPhysicalOrder : List ValueExpr) : Option (List ValueExpr) :=
  if !namedToAnonEquivalent named args anon then
    none
  else
    let instantiated := instantiateFields named.params args named.fields
    let sourceByName :=
      sourceByNameFromPhysical instantiated named.physicalToAst valuesInNamedPhysicalOrder
    anon.physicalToAst.mapM (fun astIndex => do
      let targetField <- listGet? anon.fields astIndex
      let source <- lookupValue? sourceByName targetField.name
      if source.ty == targetField.ty then
        some { source with ty := targetField.ty }
      else
        none)

def i32 : Ty := .atom 32
def u8 : Ty := .atom 8
def nParamTy : Ty := .param 1
def arrayN : Ty := .array 4 u8
def arrayMismatched : Ty := .array 3 u8

def namedPair : NamedStruct :=
  { isExtern := false
    params := []
    fields := [{ name := 1, ty := i32 }, { name := 2, ty := u8 }]
    physicalToAst := [1, 0] }

def anonPairReordered : AnonStruct :=
  { isExtern := false
    fields := [{ name := 2, ty := u8 }, { name := 1, ty := i32 }]
    physicalToAst := [0, 1] }

def externPair : NamedStruct :=
  { namedPair with isExtern := true }

def externPairSameOrder : AnonStruct :=
  { isExtern := true
    fields := [{ name := 1, ty := i32 }, { name := 2, ty := u8 }]
    physicalToAst := [0, 1] }

def externPairReordered : AnonStruct :=
  { anonPairReordered with isExtern := true }

def namedBuf : NamedStruct :=
  { isExtern := false
    params := [1]
    fields := [{ name := 7, ty := .array 1 nParamTy }]
    physicalToAst := [0] }

def anonBuf4 : AnonStruct :=
  { isExtern := false
    fields := [{ name := 7, ty := .array 1 (.array 4 u8) }]
    physicalToAst := [0] }

def anonBuf3 : AnonStruct :=
  { anonBuf4 with fields := [{ name := 7, ty := .array 1 (.array 3 u8) }] }

/-- Field order differences are allowed when names and types match. -/
example :
    namedToAnonEquivalent namedPair [] anonPairReordered = true := by
  native_decide

/-- `extern struct` decay preserves ABI field order instead of sorting by name. -/
example :
    namedToAnonEquivalent externPair [] externPairSameOrder = true := by
  native_decide

/-- Reordered `extern struct` fields are not ABI-equivalent. -/
example :
    namedToAnonEquivalent externPair [] externPairReordered = false := by
  native_decide

/-- Lowering rewrites into the anonymous target's physical order by field name. -/
example :
    (rewriteNamedToAnon?
      namedPair
      []
      anonPairReordered
      [{ field := 2, ty := u8 }, { field := 1, ty := i32 }]
      == some [{ field := 2, ty := u8 }, { field := 1, ty := i32 }]) = true := by
  native_decide

/-- Missing fields are rejected by the same equivalence used before lowering. -/
example :
    namedToAnonEquivalent
      { namedPair with fields := [{ name := 1, ty := i32 }] }
      []
      anonPairReordered = false := by
  native_decide

/-- `extern` layout markers must agree. -/
example :
    namedToAnonEquivalent { namedPair with isExtern := true } [] anonPairReordered = false := by
  native_decide

/-- Const/generic field types are compared after instantiation. -/
example :
    namedToAnonEquivalent namedBuf [.array 4 u8] anonBuf4 = true := by
  native_decide

/-- A mismatched const-instantiated field shape is rejected. -/
example :
    namedToAnonEquivalent namedBuf [.array 4 u8] anonBuf3 = false := by
  native_decide

/-- Pointer aggregate decay relies on the same element-shape equivalence. -/
example :
    namedToAnonEquivalent namedBuf [.array 4 u8] anonBuf4
      && !namedToAnonEquivalent namedBuf [.array 3 u8] anonBuf4 = true := by
  native_decide

end KernFormal.AggregateDecay
