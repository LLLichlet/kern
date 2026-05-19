/-!
Formal model for match exhaustiveness and pattern lowering.

The compiler has two independent views of a match:

* type checking lowers source patterns into a coverage matrix and asks whether
  each new row is useful and whether the final matrix is exhaustive;
* lowering emits an ordered if-chain that tests patterns in source order and
  evaluates the first matching arm.

This model keeps those views connected for the constructor fragment that Kern's
exhaustiveness checker handles exactly: booleans, enum variants, and product
payloads. User-defined `Pattern[T]` value patterns are deliberately modeled as
opaque runtime tests: they may lower to a branch condition, but they do not
contribute to constructor exhaustiveness.
-/

namespace KernFormal.PatternMatch

abbrev CtorId := Nat
abbrev BindingId := Nat
abbrev TypeId := Nat
abbrev ArmId := Nat

inductive Ty where
  | atom : TypeId -> Ty
  | bool
  | enum : List (CtorId × List Ty) -> Ty
  | prod : List Ty -> Ty
  deriving Repr, BEq

structure BindField where
  name : BindingId
  ty : Ty
  isMut : Bool
  deriving Repr, BEq

inductive Val where
  | atom : Nat -> Val
  | bool : Bool -> Val
  | ctor : CtorId -> List Val -> Val
  | prod : List Val -> Val
  deriving Repr, BEq

inductive Pat where
  | wildcard
  | bind : BindingId -> Pat
  | bool : Bool -> Pat
  | ctor : CtorId -> List Pat -> Pat
  | prod : List Pat -> Pat
  | prodPartial : List (Nat × Pat) -> Pat
  | opaqueUser : List BindField -> Pat
  deriving Repr, BEq

structure Ctor where
  id : CtorId
  args : List Ty
  deriving Repr

def listGet? : List α -> Nat -> Option α
  | [], _ => none
  | value :: _, 0 => some value
  | _ :: rest, n + 1 => listGet? rest n

/-- The constructor universe used by the coverage checker. -/
def constructorsOf : Ty -> Option (List Ctor)
  | .bool =>
      some
        [ ({ id := 0, args := [] } : Ctor),
          ({ id := 1, args := [] } : Ctor) ]
  | .enum ctors =>
      some (ctors.map (fun ctor => { id := ctor.fst, args := ctor.snd }))
  | .prod fields => some [({ id := 0, args := fields } : Ctor)]
  | .atom _ => none

def valueMatchesCtor (ctorId : CtorId) : Val -> Option (List Val)
  | .bool value =>
      if ctorId == 0 && value == false then some []
      else if ctorId == 1 && value == true then some []
      else none
  | .ctor id args => if id == ctorId then some args else none
  | .prod fields => if ctorId == 0 then some fields else none
  | .atom _ => none

/-- Runtime pattern semantics used by lowering's if-chain. -/
partial def patMatches (pat : Pat) (value : Val) : Bool :=
  match pat, value with
  | .wildcard, _ => true
  | .bind _, _ => true
  | .opaqueUser _, _ => false
  | .bool expected, .bool actual => expected == actual
  | .ctor ctorId args, value =>
      match valueMatchesCtor ctorId value with
      | some values => listAll2 args values
      | none => false
  | .prod args, .prod values => listAll2 args values
  | .prodPartial fields, .prod values =>
      fields.all (fun field =>
        match listGet? values field.fst with
        | some value => patMatches field.snd value
        | none => false)
  | _, _ => false
where
  listAll2 : List Pat -> List Val -> Bool
    | [], [] => true
    | pat :: pats, value :: values => patMatches pat value && listAll2 pats values
    | _, _ => false

partial def bindingsOf : Pat -> List BindingId
  | .wildcard => []
  | .bind id => [id]
  | .bool _ => []
  | .ctor _ args => args.flatMap bindingsOf
  | .prod fields => fields.flatMap bindingsOf
  | .prodPartial fields => fields.flatMap (fun field => bindingsOf field.snd)
  | .opaqueUser fields => fields.map BindField.name

def ctorArgTys? (id : CtorId) (ctors : List Ctor) : Option (List Ty) :=
  ctors.find? (fun ctor => ctor.id == id) |>.map Ctor.args

partial def insertBindField (field : BindField) : List BindField -> List BindField
  | [] => [field]
  | head :: rest =>
      if field.name <= head.name then field :: head :: rest
      else head :: insertBindField field rest

/-- Canonical binding order, matching the compiler's name-sorted bind shape. -/
def canonicalBindFields (fields : List BindField) : List BindField :=
  fields.foldl (fun sorted field => insertBindField field sorted) []

mutual
  partial def bindingFieldsAligned : List Ty -> List Pat -> List BindField
    | [], [] => []
    | ty :: tys, pat :: pats => bindingFieldsOf ty pat ++ bindingFieldsAligned tys pats
    | _, _ => []

  /-- Binding fields made visible to a match arm body.

  The compiler computes this shape from the target type, then requires all
  alternatives in one arm to produce the same sorted `(name, type, mutability)`
  list before it defines the bindings for the shared body.
  -/
  partial def bindingFieldsOf (target : Ty) (pat : Pat) : List BindField :=
    match target, pat with
    | _, .wildcard => []
    | target, .bind id => [{ name := id, ty := target, isMut := false }]
    | _, .bool _ => []
    | .enum ctors, .ctor id args =>
        match ctorArgTys? id (ctors.map (fun ctor => ({ id := ctor.fst, args := ctor.snd } : Ctor))) with
        | some argTys => bindingFieldsAligned argTys args
        | none => []
    | .prod fieldTys, .prod fields => bindingFieldsAligned fieldTys fields
    | .prod fieldTys, .prodPartial fields =>
        fields.flatMap (fun field =>
          match listGet? fieldTys field.fst with
          | some fieldTy => bindingFieldsOf fieldTy field.snd
          | none => [])
    | _, .ctor _ args => args.flatMap (bindingFieldsOf target)
    | _, .prod fields => fields.flatMap (bindingFieldsOf target)
    | _, .prodPartial fields => fields.flatMap (fun field => bindingFieldsOf target field.snd)
    | _, .opaqueUser fields => fields
end

def bindingShape (target : Ty) (pat : Pat) : List BindField :=
  canonicalBindFields (bindingFieldsOf target pat)

def sameArmBindings (target : Ty) : List Pat -> Bool
  | [] => true
  | pat :: rest =>
      let expected := bindingShape target pat
      rest.all (fun alt => bindingShape target alt == expected)

inductive CoveragePattern where
  | wildcard
  | ctor : CtorId -> List CoveragePattern -> CoveragePattern
  deriving Repr

partial def coveragePatternEq : CoveragePattern -> CoveragePattern -> Bool
  | .wildcard, .wildcard => true
  | .ctor leftId leftArgs, .ctor rightId rightArgs =>
      leftId == rightId && listAll2 leftArgs rightArgs
  | _, _ => false
where
  listAll2 : List CoveragePattern -> List CoveragePattern -> Bool
    | [], [] => true
    | left :: leftRest, right :: rightRest =>
        coveragePatternEq left right && listAll2 leftRest rightRest
    | _, _ => false

instance : BEq CoveragePattern where
  beq := coveragePatternEq

abbrev Matrix := List (List CoveragePattern)

mutual
  partial def coverageLowerAligned : List Ty -> List Pat -> Option (List CoveragePattern)
    | [], [] => some []
    | ty :: tys, pat :: pats => do
        let head <- coverageLower ty pat
        let rest <- coverageLowerAligned tys pats
        some (head :: rest)
    | _, _ => none

  partial def coverageLowerPartialProduct
      (fieldTys : List Ty)
      (fields : List (Nat × Pat))
      (index : Nat) : Option (List CoveragePattern) :=
    match fieldTys with
    | [] => some []
    | fieldTy :: restTys => do
        let head <-
          match fields.find? (fun field => field.fst == index) with
          | some field => coverageLower fieldTy field.snd
          | none => some .wildcard
        let rest <- coverageLowerPartialProduct restTys fields (index + 1)
        some (head :: rest)

  /-- Type-checker lowering into the exact coverage fragment.

  This takes the target type for the same reason the compiler does:
  enum payloads and product fields determine how nested subpatterns are lowered,
  and omitted product fields become wildcard coverage entries.
  -/
  partial def coverageLower : Ty -> Pat -> Option CoveragePattern
    | _, .wildcard => some .wildcard
    | _, .bind _ => some .wildcard
    | .bool, .bool value => some (.ctor (if value then 1 else 0) [])
    | .enum ctors, .ctor id args =>
        match ctorArgTys? id (ctors.map (fun ctor => ({ id := ctor.fst, args := ctor.snd } : Ctor))) with
        | some argTys =>
            coverageLowerAligned argTys args |>.map (fun lowered => .ctor id lowered)
        | none => none
    | .prod fieldTys, .prod fields =>
        coverageLowerAligned fieldTys fields |>.map (fun lowered => .ctor 0 lowered)
    | .prod fieldTys, .prodPartial fields =>
        coverageLowerPartialProduct fieldTys fields 0 |>.map (fun lowered => .ctor 0 lowered)
    | _, .opaqueUser _ => none
    | _, _ => none
end

def specializePattern (pat : CoveragePattern) (ctor : Ctor) : Option (List CoveragePattern) :=
  match pat with
  | .wildcard => some (List.replicate ctor.args.length .wildcard)
  | .ctor id args => if id == ctor.id then some args else none

def defaultMatrix (matrix : Matrix) : Matrix :=
  matrix.filterMap (fun row =>
    match row with
    | .wildcard :: rest => some rest
    | _ => none)

def specializeMatrix (matrix : Matrix) (ctor : Ctor) : Matrix :=
  matrix.filterMap (fun row =>
    match row with
    | [] => none
    | head :: rest =>
        specializePattern head ctor |>.map (fun specialized => specialized ++ rest))

def hasWildcardCover (matrix : Matrix) (width : Nat) : Bool :=
  matrix.any (fun row => row.length == width && row.all (fun pat => pat == .wildcard))

partial def vectorUseful (tys : List Ty) (matrix : Matrix) (vector : List CoveragePattern) : Bool :=
  match tys, vector with
  | [], [] => matrix.isEmpty
  | headTy :: restTys, headPat :: restVector =>
      if hasWildcardCover matrix (headTy :: restTys).length then
        false
      else
        match constructorsOf headTy with
        | some ctors =>
            match headPat with
            | .wildcard =>
                ctors.any (fun ctor =>
                  vectorUseful (ctor.args ++ restTys)
                    (specializeMatrix matrix ctor)
                    (List.replicate ctor.args.length .wildcard ++ restVector))
            | .ctor id args =>
                match ctors.find? (fun ctor => ctor.id == id) with
                | some ctor =>
                    vectorUseful (ctor.args ++ restTys)
                      (specializeMatrix matrix ctor)
                      (args ++ restVector)
                | none => false
        | none =>
            match headPat with
            | .wildcard => vectorUseful restTys (defaultMatrix matrix) restVector
            | .ctor _ _ => false
  | _, _ => false

def addUseful? (ty : Ty) (matrix : Matrix) (pat : Pat) : Option Matrix :=
  coverageLower ty pat >>= fun lowered =>
    if vectorUseful [ty] matrix [lowered] then some (matrix ++ [[lowered]]) else none

def buildMatrix (ty : Ty) (patterns : List Pat) : Matrix :=
  patterns.foldl (fun matrix pat =>
    match addUseful? ty matrix pat with
    | some matrix => matrix
    | none => matrix) []

mutual
  partial def firstUncoveredCtor
      (ctors : List Ctor)
      (restTys : List Ty)
      (matrix : Matrix) : Option (List CoveragePattern) :=
    match ctors with
    | [] => none
    | ctor :: rest =>
        match findUncovered (ctor.args ++ restTys) (specializeMatrix matrix ctor) with
        | some uncovered =>
            some (.ctor ctor.id (uncovered.take ctor.args.length) :: uncovered.drop ctor.args.length)
        | none => firstUncoveredCtor rest restTys matrix

  partial def findUncovered (tys : List Ty) (matrix : Matrix) : Option (List CoveragePattern) :=
    match tys with
    | [] => if matrix.isEmpty then some [] else none
    | headTy :: restTys =>
        if hasWildcardCover matrix (headTy :: restTys).length then
          none
        else
          match constructorsOf headTy with
          | some ctors => firstUncoveredCtor ctors restTys matrix
          | none =>
              findUncovered restTys (defaultMatrix matrix)
                |>.map (fun rest => .wildcard :: rest)
end

def exhaustive (ty : Ty) (patterns : List Pat) : Bool :=
  (findUncovered [ty] (buildMatrix ty patterns)).isNone

/--
`let PAT = init else { ... }` checks failure arms against the complement of
the success pattern.  This mirrors the compiler by inserting the primary
success pattern into the coverage matrix before adding failure-arm patterns.
-/
def letElseFailureCovered (ty : Ty) (primary : Pat) (failureArms : List Pat) : Bool :=
  exhaustive ty (primary :: failureArms)

def letElseFirstUncovered
    (ty : Ty)
    (primary : Pat)
    (failureArms : List Pat) : Option (List CoveragePattern) :=
  findUncovered [ty] (buildMatrix ty (primary :: failureArms))

def letElseArmUseful
    (ty : Ty)
    (primary : Pat)
    (previousFailureArms : List Pat)
    (arm : Pat) : Bool :=
  match coverageLower ty arm with
  | some lowered => vectorUseful [ty] (buildMatrix ty (primary :: previousFailureArms)) [lowered]
  | none => true

partial def lowerFirstMatch (value : Val) (arms : List (Pat × ArmId)) : Option ArmId :=
  match arms with
  | [] => none
  | (pat, armId) :: rest =>
      if patMatches pat value then some armId else lowerFirstMatch value rest

def loweredConditionSequence (arms : List (Pat × ArmId)) : List Pat :=
  arms.map Prod.fst

structure FieldDefault where
  index : Nat
  pat : Pat
  deriving Repr, BEq

def defaultFor? (defaults : List FieldDefault) (index : Nat) : Option Pat :=
  defaults.find? (fun default => default.index == index) |>.map FieldDefault.pat

partial def completeValueProductPattern
    (fieldTys : List Ty)
    (written : List (Nat × Pat))
    (defaults : List FieldDefault)
    (index : Nat) : Option (List Pat) :=
  match fieldTys with
  | [] => some []
  | _ :: restTys => do
      let head <-
        match written.find? (fun field => field.fst == index) with
        | some field => some field.snd
        | none => defaultFor? defaults index
      let rest <- completeValueProductPattern restTys written defaults (index + 1)
      some (head :: rest)

/-- Compiler-known struct value patterns match a complete literal value.

Unlike source destructuring patterns, omitted fields are not wildcards. They
must be expanded from struct field defaults before both coverage and lowering
look at the field list.
-/
def completeValuePattern
    (fieldTys : List Ty)
    (written : List (Nat × Pat))
    (defaults : List FieldDefault) : Option Pat :=
  completeValueProductPattern fieldTys written defaults 0 |>.map Pat.prod

def coverageLowerValueProduct
    (fieldTys : List Ty)
    (written : List (Nat × Pat))
    (defaults : List FieldDefault) : Option CoveragePattern :=
  completeValuePattern fieldTys written defaults >>= coverageLower (.prod fieldTys)

def lowerValueProductMatches
    (fieldTys : List Ty)
    (written : List (Nat × Pat))
    (defaults : List FieldDefault)
    (value : Val) : Bool :=
  match completeValuePattern fieldTys written defaults with
  | some pat => patMatches pat value
  | none => false

def boolTy : Ty := .bool
def optionBoolTy : Ty :=
  .enum
    [ (10, []),
      (11, [Ty.bool]) ]
def pairBoolTy : Ty := .prod [.bool, .bool]
def pairBoolVal : Val := .prod [.bool false, .bool true]
def pairI32BoolTy : Ty := .prod [.atom 32, .bool]
def pairDefaultRightFalse : List FieldDefault :=
  [{ index := 1, pat := .bool false }]

def nonePat : Pat := .ctor 10 []
def someFalsePat : Pat := .ctor 11 [.bool false]
def someTruePat : Pat := .ctor 11 [.bool true]
def someAnyPat : Pat := .ctor 11 [.wildcard]

/-- Bool constructor coverage is exhaustive exactly when both constructors appear. -/
example : exhaustive boolTy [.bool false, .bool true] = true := by
  native_decide

/-- Missing a bool constructor leaves an uncovered witness. -/
example : exhaustive boolTy [.bool false] = false := by
  native_decide

/-- Nested enum payload coverage distinguishes `Some(false)` from `Some(true)`. -/
example : exhaustive optionBoolTy [nonePat, someFalsePat] = false := by
  native_decide

/-- A wildcard payload covers both nested bool constructors. -/
example : exhaustive optionBoolTy [nonePat, someAnyPat] = true := by
  native_decide

/-- Struct/product coverage requires each product field to be covered. -/
example :
    exhaustive pairBoolTy
      [.prod [.bool false, .wildcard],
       .prod [.bool true, .bool false]] = false := by
  native_decide

/-- Source struct patterns may omit fields; coverage treats omissions as wildcards. -/
example :
    (coverageLower pairBoolTy (.prodPartial [(0, .bool false)]) ==
      some (.ctor 0 [.ctor 0 [], .wildcard])) = true := by
  native_decide

/-- Lowering tests only written struct fields, equivalent to wildcard omissions. -/
example :
    patMatches (.prodPartial [(0, .bool false)]) pairBoolVal = true := by
  native_decide

/-- Struct value-pattern omissions are completed from defaults, not wildcards. -/
example :
    (completeValuePattern [.atom 32, .bool] [(0, .bind 9)] pairDefaultRightFalse ==
      some (.prod [.bind 9, .bool false])) = true := by
  native_decide

/-- Coverage for a partial struct value pattern includes the default field test. -/
example :
    (coverageLowerValueProduct [.atom 32, .bool] [(0, .wildcard)] pairDefaultRightFalse ==
      some (.ctor 0 [.wildcard, .ctor 0 []])) = true := by
  native_decide

/-- Runtime lowering must reject values whose omitted field differs from the default. -/
example :
    lowerValueProductMatches [.atom 32, .bool] [(0, .wildcard)] pairDefaultRightFalse
      (.prod [.atom 1, .bool true]) = false := by
  native_decide

/-- Runtime lowering accepts the same value-pattern when the defaulted field matches. -/
example :
    lowerValueProductMatches [.atom 32, .bool] [(0, .wildcard)] pairDefaultRightFalse
      (.prod [.atom 1, .bool false]) = true := by
  native_decide

/-- User patterns are opaque for exhaustiveness even if they may match at runtime. -/
example : exhaustive boolTy [.opaqueUser []] = false := by
  native_decide

/-- Once a wildcard row is present, later constructor rows are not useful. -/
example :
    addUseful? boolTy (buildMatrix boolTy [.wildcard]) (.bool true) = none := by
  native_decide

/-- Lowering preserves source order by selecting the first matching arm. -/
example :
    lowerFirstMatch (.bool true) [(.wildcard, 0), (.bool true, 1)] = some 0 := by
  native_decide

/-- Bindings are attached only to the arm whose pattern matched. -/
example :
    bindingsOf (.ctor 11 [.bind 7]) = [7] := by
  native_decide

/-- Alternative patterns in one arm may share a canonical binding shape. -/
example :
    sameArmBindings pairBoolTy
      [.prodPartial [(0, .bind 2), (1, .wildcard)],
       .prodPartial [(1, .wildcard), (0, .bind 2)]] = true := by
  native_decide

/-- Alternative patterns with different names/types must not share one body scope. -/
example :
    sameArmBindings pairBoolTy
      [.prodPartial [(0, .bind 2)],
       .prodPartial [(1, .bind 3)]] = false := by
  native_decide

/-- Opaque user patterns keep their `Pattern[T]::Bind` shape but not coverage. -/
example :
    sameArmBindings boolTy
      [.opaqueUser [{ name := 4, ty := .bool, isMut := false }],
       .opaqueUser [{ name := 4, ty := .bool, isMut := false }]] = true := by
  native_decide

/-- Distinct user-pattern bind shapes are rejected before the body is checked. -/
example :
    sameArmBindings boolTy
      [.opaqueUser [{ name := 4, ty := .bool, isMut := false }],
       .opaqueUser [{ name := 5, ty := .bool, isMut := false }]] = false := by
  native_decide

/-- A let-else arm block covers exactly the primary pattern's complement. -/
example :
    letElseFailureCovered optionBoolTy someAnyPat [nonePat] = true := by
  native_decide

/-- Nested complements still require all payload cases not matched by success. -/
example :
    letElseFailureCovered optionBoolTy someFalsePat [nonePat] = false := by
  native_decide

/-- Adding the remaining nested payload case closes the failure domain. -/
example :
    letElseFailureCovered optionBoolTy someFalsePat [nonePat, someTruePat] = true := by
  native_decide

/-- Failure arms already covered by the success pattern are unreachable. -/
example :
    letElseArmUseful optionBoolTy someAnyPat [] someTruePat = false := by
  native_decide

/-- Failure arms that cover the remaining complement are useful. -/
example :
    letElseArmUseful optionBoolTy someAnyPat [] nonePat = true := by
  native_decide

/--
Opaque user patterns do not contribute to constructor coverage, so let-else
failure arms must conservatively cover the whole target domain.
-/
example :
    letElseFailureCovered boolTy (.opaqueUser []) [.bool false] = false := by
  native_decide

/-- A catch-all failure arm closes the domain after an opaque primary pattern. -/
example :
    letElseFailureCovered boolTy (.opaqueUser []) [.wildcard] = true := by
  native_decide

end KernFormal.PatternMatch
