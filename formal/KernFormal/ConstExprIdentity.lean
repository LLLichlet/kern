/-!
Formal model for const-generic expression identity.

Kern represents const generics as values, named params, or interned expression
nodes.  The soundness boundary is deliberately conservative:

* concrete expressions may fold to values and become canonical cache keys;
* expressions containing params remain symbolic until substitution can revisit
  them;
* direct generic inference can bind a plain const parameter, but it must not
  invert symbolic expressions such as `N + 1`;
* imported source snapshots and monomorphization keys must preserve const
  expression shape whenever an expression cannot be folded to a value.
-/

namespace KernFormal.ConstExprIdentity

abbrev ParamId := Nat
abbrev DefId := Nat

inductive ConstTy where
  | int : Nat -> ConstTy
  | bool
  deriving Repr, BEq

inductive ConstExpr where
  | int : Int -> ConstTy -> ConstExpr
  | bool : Bool -> ConstExpr
  | param : ParamId -> ConstTy -> ConstExpr
  | add : ConstExpr -> ConstExpr -> ConstTy -> ConstExpr
  | div : ConstExpr -> ConstExpr -> ConstTy -> ConstExpr
  | cast : ConstExpr -> ConstTy -> ConstExpr
  deriving Repr, BEq

inductive ConstKey where
  | valueInt : Int -> ConstTy -> ConstKey
  | valueBool : Bool -> ConstKey
  | param : ParamId -> ConstTy -> ConstKey
  | expr : ConstExpr -> ConstKey
  deriving Repr, BEq, Inhabited

structure MonoKey where
  defn : DefId
  constArgs : List ConstKey
  deriving Repr, BEq, Inhabited

def ConstExpr.ty? : ConstExpr -> Option ConstTy
  | .int _ ty => some ty
  | .bool _ => some .bool
  | .param _ ty => some ty
  | .add _ _ ty => some ty
  | .div _ _ ty => some ty
  | .cast _ ty => some ty

partial def ConstExpr.containsParam : ConstExpr -> Bool
  | .int _ _ => false
  | .bool _ => false
  | .param _ _ => true
  | .add left right _ => left.containsParam || right.containsParam
  | .div left right _ => left.containsParam || right.containsParam
  | .cast expr _ => expr.containsParam

def ConstKey.containsParam : ConstKey -> Bool
  | .valueInt _ _ => false
  | .valueBool _ => false
  | .param _ _ => true
  | .expr body => body.containsParam

def ConstKey.ty? : ConstKey -> Option ConstTy
  | .valueInt _ ty => some ty
  | .valueBool _ => some .bool
  | .param _ ty => some ty
  | .expr body => body.ty?

def asInt? : ConstKey -> Option (Int × ConstTy)
  | .valueInt value ty => some (value, ty)
  | _ => none

def valueToTy (value : Int) (ty : ConstTy) : Option ConstKey :=
  match ty with
  | .int _ => some (.valueInt value ty)
  | .bool => none

partial def foldConst : ConstExpr -> ConstKey
  | .int value ty => .valueInt value ty
  | .bool value => .valueBool value
  | .param name ty => .param name ty
  | .add left right ty =>
      match asInt? (foldConst left), asInt? (foldConst right) with
      | some (leftValue, _), some (rightValue, _) =>
          match valueToTy (leftValue + rightValue) ty with
          | some value => value
          | none => .expr (.add left right ty)
      | _, _ => .expr (.add left right ty)
  | .div left right ty =>
      match asInt? (foldConst left), asInt? (foldConst right) with
      | some (leftValue, _), some (rightValue, _) =>
          if rightValue == 0 then
            .expr (.div left right ty)
          else
            match valueToTy (leftValue / rightValue) ty with
            | some value => value
            | none => .expr (.div left right ty)
      | _, _ => .expr (.div left right ty)
  | .cast body ty =>
      match asInt? (foldConst body) with
      | some (value, _) =>
          match valueToTy value ty with
          | some value => value
          | none => .expr (.cast body ty)
      | none => .expr (.cast body ty)

def canonicalConstKey (key : ConstKey) : ConstKey :=
  match key with
  | .expr body => foldConst body
  | other => other

def canonicalMonoKey (key : MonoKey) : MonoKey :=
  { key with constArgs := key.constArgs.map canonicalConstKey }

def lookupConst? : List (ParamId × ConstKey) -> ParamId -> Option ConstKey
  | [], _ => none
  | binding :: rest, name =>
      if binding.1 == name then some binding.2 else lookupConst? rest name

partial def substituteConst (subst : List (ParamId × ConstKey)) : ConstExpr -> ConstExpr
  | .int value ty => .int value ty
  | .bool value => .bool value
  | .param name ty =>
      match lookupConst? subst name with
      | some (.valueInt value valueTy) => .int value valueTy
      | some (.valueBool value) => .bool value
      | some (.param next ty) => .param next ty
      | some (.expr body) => body
      | none => .param name ty
  | .add left right ty => .add (substituteConst subst left) (substituteConst subst right) ty
  | .div left right ty => .div (substituteConst subst left) (substituteConst subst right) ty
  | .cast body ty => .cast (substituteConst subst body) ty

partial def occurs (needle : ParamId) : ConstExpr -> Bool
  | .int _ _ => false
  | .bool _ => false
  | .param name _ => name == needle
  | .add left right _ => occurs needle left || occurs needle right
  | .div left right _ => occurs needle left || occurs needle right
  | .cast body _ => occurs needle body

def keyOccurs (needle : ParamId) : ConstKey -> Bool
  | .valueInt _ _ => false
  | .valueBool _ => false
  | .param name _ => name == needle
  | .expr body => occurs needle body

/-- Direct call-signature inference for const generics.

The generic side may be a plain param.  Symbolic expressions are matched only
after substitution/folding makes them exactly equal to the concrete side; they
are not algebraically inverted.
-/
def directInfer? (generic concrete : ConstKey) : Option (ParamId × ConstKey) :=
  if generic.ty? != concrete.ty? then
    none
  else
    match generic with
    | .param name _ =>
        if keyOccurs name concrete then none else some (name, concrete)
    | .expr body =>
        if canonicalConstKey (.expr body) == canonicalConstKey concrete then some (0, concrete)
        else none
    | _ =>
        if canonicalConstKey generic == canonicalConstKey concrete then some (0, concrete)
        else none

def usizeTy : ConstTy := .int 64
def nParam : ConstExpr := .param 1 usizeTy
def nPlusOne : ConstExpr := .add nParam (.int 1 usizeTy) usizeTy
def twoPlusTwo : ConstExpr := .add (.int 2 usizeTy) (.int 2 usizeTy) usizeTy
def divByZero : ConstExpr := .div (.int 4 usizeTy) (.int 0 usizeTy) usizeTy

def keyLen4 : MonoKey := { defn := 10, constArgs := [.valueInt 4 usizeTy] }
def keyLenTwoPlusTwo : MonoKey := { defn := 10, constArgs := [.expr twoPlusTwo] }
def keyLenNPlusOne : MonoKey := { defn := 10, constArgs := [.expr nPlusOne] }
def keyLenN : MonoKey := { defn := 10, constArgs := [.param 1 usizeTy] }

/-- Concrete const expressions fold to value identity. -/
example :
    (canonicalConstKey (.expr twoPlusTwo) == .valueInt 4 usizeTy) = true := by
  native_decide

/-- Folded const expressions share monomorphization identity with their value. -/
example :
    canonicalMonoKey keyLenTwoPlusTwo == canonicalMonoKey keyLen4 := by
  native_decide

/-- Expressions containing params remain open and keep expression identity. -/
example :
    canonicalConstKey (.expr nPlusOne) == .expr nPlusOne := by
  native_decide

/-- Division by zero is deliberately left symbolic for diagnostics. -/
example :
    canonicalConstKey (.expr divByZero) == .expr divByZero := by
  native_decide

/-- Direct inference may bind a plain const parameter to a concrete value. -/
example :
    (directInfer? (.param 1 usizeTy) (.valueInt 4 usizeTy) ==
      some (1, .valueInt 4 usizeTy)) = true := by
  native_decide

/-- But direct inference must not invert `N + 1` to solve for `N`. -/
example :
    directInfer? (.expr nPlusOne) (.valueInt 5 usizeTy) = none := by
  native_decide

/-- After an earlier substitution, `N + 1` may fold and match directly. -/
example :
    canonicalConstKey (.expr (substituteConst [(1, .valueInt 4 usizeTy)] nPlusOne))
      == .valueInt 5 usizeTy := by
  native_decide

/-- Open `N + 1` and open `N` are distinct monomorphization identities. -/
example :
    (canonicalMonoKey keyLenNPlusOne == canonicalMonoKey keyLenN) = false := by
  native_decide

/-- Imported/source snapshots must preserve an open const expression shape. -/
example :
    ((canonicalMonoKey keyLenNPlusOne).constArgs == [.expr nPlusOne]) = true := by
  native_decide

end KernFormal.ConstExprIdentity
