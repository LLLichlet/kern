import KernFormal.FakeRefl

/-!
Formal model for trait impl coherence and orphan-rule boundaries.

This file focuses on the cross-module and const-generic cases that have been
bug-prone in the compiler:

* const generic arguments participate in impl-head overlap;
* disjoint const literals keep impls coherent;
* an overlap is accepted only when one impl head strictly specializes the other;
* imported/foreign traits may be implemented only for targets anchored by a
  local type, directly or through builtin wrappers such as pointers and arrays.

Compiler correspondence:
`validate_trait_impl_coherence`, `overlapping_trait_impl_pair`,
`freshen_impl_head_types_for_overlap`, `compare_impl_specificity`, and
`trait_impl_is_orphan_legal` in `compiler/kernc_sema/src/passes/types`.
-/

namespace KernFormal.Coherence

open KernFormal.FakeRefl

abbrev ModuleId := Nat

inductive Locality where
  | local : ModuleId -> Locality
  | foreign : ModuleId -> Locality
  deriving DecidableEq, Repr

inductive HeadPat where
  | tyAtom : Nat -> HeadPat
  | tyParam : Nat -> HeadPat
  | constLit : Nat -> HeadPat
  | constParam : Nat -> HeadPat
  deriving DecidableEq, Repr

def HeadPat.isParam : HeadPat -> Bool
  | .tyParam _ => true
  | .constParam _ => true
  | _ => false

/-- Unification for freshened impl-head arguments.

This abstracts the overlap check after each impl's generic params have been
freshened. Equal literals overlap, different literals are disjoint, and a fresh
parameter can overlap either side. -/
def HeadPat.unifies (left right : HeadPat) : Bool :=
  match left, right with
  | .tyAtom a, .tyAtom b => a == b
  | .constLit a, .constLit b => a == b
  | .tyParam _, .tyAtom _ => true
  | .tyAtom _, .tyParam _ => true
  | .tyParam _, .tyParam _ => true
  | .constParam _, .constLit _ => true
  | .constLit _, .constParam _ => true
  | .constParam _, .constParam _ => true
  | _, _ => false

/-- A specialized pattern matches a general pattern when the general side may bind.

This mirrors `impl_head_specializes`: the general impl is matched as flexible,
while the specialized impl is rigid. -/
def HeadPat.specializes (specialized general : HeadPat) : Bool :=
  match specialized, general with
  | .tyAtom a, .tyAtom b => a == b
  | .constLit a, .constLit b => a == b
  | .tyAtom _, .tyParam _ => true
  | .constLit _, .constParam _ => true
  | .tyParam a, .tyParam b => a == b
  | .constParam a, .constParam b => a == b
  | _, _ => false

structure ImplHead where
  target : List HeadPat
  trait : List HeadPat
  whereCount : Nat
  deriving DecidableEq, Repr

def listAll2 (f : α -> β -> Bool) : List α -> List β -> Bool
  | [], [] => true
  | left :: leftRest, right :: rightRest => f left right && listAll2 f leftRest rightRest
  | _, _ => false

def headsOverlap (left right : ImplHead) : Bool :=
  listAll2 HeadPat.unifies left.target right.target
    && listAll2 HeadPat.unifies left.trait right.trait

def headSpecializes (specialized general : ImplHead) : Bool :=
  listAll2 HeadPat.specializes specialized.target general.target
    && listAll2 HeadPat.specializes specialized.trait general.trait
    && (specialized.target != general.target
      || specialized.trait != general.trait
      || general.whereCount < specialized.whereCount)

inductive Specificity where
  | leftMoreSpecific
  | rightMoreSpecific
  | ambiguous
  deriving DecidableEq, Repr

def compareSpecificity (left right : ImplHead) : Specificity :=
  match headSpecializes left right, headSpecializes right left with
  | true, false => .leftMoreSpecific
  | false, true => .rightMoreSpecific
  | true, true =>
      if right.whereCount < left.whereCount then .leftMoreSpecific
      else if left.whereCount < right.whereCount then .rightMoreSpecific
      else .ambiguous
  | _, _ => .ambiguous

def coherentPair (left right : ImplHead) : Bool :=
  if headsOverlap left right then
    match compareSpecificity left right with
    | .leftMoreSpecific | .rightMoreSpecific => true
    | .ambiguous => false
  else
    true

inductive AnchorTy where
  | primitive : Nat -> AnchorTy
  | localDef : ModuleId -> AnchorTy
  | foreignDef : ModuleId -> AnchorTy
  | ptr : AnchorTy -> AnchorTy
  | slice : AnchorTy -> AnchorTy
  | array : AnchorTy -> ConstArg -> AnchorTy
  | projection : AnchorTy
  deriving DecidableEq, Repr

/-- Local anchors pass through builtin wrappers but not through projections.

This mirrors `type_has_local_impl_anchor`: pointers, slices, and arrays recurse;
primitive and projected targets do not anchor a foreign trait impl. -/
def hasLocalAnchor (home : ModuleId) : AnchorTy -> Bool
  | .localDef moduleId => moduleId == home
  | .foreignDef _ => false
  | .ptr elem => hasLocalAnchor home elem
  | .slice elem => hasLocalAnchor home elem
  | .array elem _ => hasLocalAnchor home elem
  | .primitive _ => false
  | .projection => false

def orphanLegal
    (implHome : ModuleId)
    (traitLocality : Locality)
    (target : AnchorTy) : Bool :=
  match traitLocality with
  | .local moduleId => moduleId == implHome
  | .foreign _ => hasLocalAnchor implHome target

def traitMarker1 : List HeadPat := [.constLit 1]
def traitMarker2 : List HeadPat := [.constLit 2]
def traitMarkerN : List HeadPat := [.constParam 0]
def targetX : List HeadPat := [.tyAtom 10]
def targetBox1 : List HeadPat := [.tyAtom 20, .constLit 1]
def targetBox2 : List HeadPat := [.tyAtom 20, .constLit 2]
def targetBoxN : List HeadPat := [.tyAtom 20, .constParam 0]

def implMarker1 : ImplHead := { target := targetX, trait := traitMarker1, whereCount := 0 }
def implMarker2 : ImplHead := { target := targetX, trait := traitMarker2, whereCount := 0 }
def implMarkerN : ImplHead := { target := targetX, trait := traitMarkerN, whereCount := 0 }
def implBox1 : ImplHead := { target := targetBox1, trait := [], whereCount := 0 }
def implBox2 : ImplHead := { target := targetBox2, trait := [], whereCount := 0 }
def implBoxN : ImplHead := { target := targetBoxN, trait := [], whereCount := 0 }
def implBox4 : ImplHead := { target := [.tyAtom 20, .constLit 4], trait := [], whereCount := 0 }
def implPair0N : ImplHead :=
  { target := [.tyAtom 30, .constLit 0, .constParam 0], trait := [], whereCount := 0 }
def implPairN0 : ImplHead :=
  { target := [.tyAtom 30, .constParam 1, .constLit 0], trait := [], whereCount := 0 }

/-- Impls differing only in target const literals are disjoint. -/
example : headsOverlap implBox1 implBox2 = false := by
  native_decide

/-- Impls differing only in trait const literals are disjoint. -/
example : headsOverlap implMarker1 implMarker2 = false := by
  native_decide

/-- A concrete const impl overlaps a generic const impl. -/
example : headsOverlap implBox4 implBoxN = true := by
  native_decide

/-- The concrete const impl is strictly more specific than the generic const impl. -/
example : compareSpecificity implBox4 implBoxN = .leftMoreSpecific := by
  native_decide

/-- Two diagonal const impls overlap at `[0, 0]` but neither specializes the other. -/
example : coherentPair implPair0N implPairN0 = false := by
  native_decide

/-- A foreign trait can be implemented for a pointer to a local type. -/
example : orphanLegal 1 (.foreign 2) (.ptr (.localDef 1)) = true := by
  native_decide

/-- A foreign trait cannot be implemented for a primitive target. -/
example : orphanLegal 1 (.foreign 2) (.primitive 32) = false := by
  native_decide

/-- A foreign trait cannot be implemented for a foreign target. -/
example : orphanLegal 1 (.foreign 2) (.foreignDef 2) = false := by
  native_decide

/-- Local anchoring passes through arrays even when const args are present. -/
example : orphanLegal 1 (.foreign 2) (.array (.localDef 1) (.lit 4)) = true := by
  native_decide

/-- Projections do not count as local anchors for foreign orphan impls. -/
example : orphanLegal 1 (.foreign 2) .projection = false := by
  native_decide

end KernFormal.Coherence
