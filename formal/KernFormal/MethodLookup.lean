import KernFormal.FakeRefl

/-!
Formal model for Kern method lookup across inherent impls, trait impls, active
generic bounds, and trait default methods.

This is an executable specification for the source-order and ambiguity rules in
`compiler/kernc_sema/src/query/named.rs` and
`compiler/kernc_sema/src/query/methods.rs`:

* method sources are queried in compiler order;
* the first source that finds a unique method wins;
* an ambiguous source stops lookup instead of falling through;
* impl/default candidates use the same maximal-specificity frontier as trait
  proof/projection queries;
* impl methods are exposed only after all local type and const generics were
  solved from the receiver and where-clauses.
-/

namespace KernFormal.MethodLookup

open KernFormal.FakeRefl

abbrev MethodId := Nat

/-- One applicable method candidate after receiver matching.

`implArgs` corresponds to the generic arguments returned by
`MemberQuery::resolve_impl_applicability`.  The compiler rejects the candidate
when any argument is still a type parameter or symbolic const expression; that
is modeled by `MethodCandidate.argsResolved` below. -/
structure MethodCandidate where
  impl : ImplId
  method : MethodId
  result : Ty
  implArgs : List GenericArg
  specificity : Nat
  deriving DecidableEq, Repr

instance : BEq MethodCandidate where
  beq left right :=
    left.impl == right.impl
      && left.method == right.method
      && left.result == right.result
      && left.implArgs == right.implArgs
      && left.specificity == right.specificity

/-- Compiler-visible applicability requires all inferred impl args to be closed.

This mirrors `impl_generic_args_fully_resolved` after
`resolve_impl_applicability`, including const-generic expressions such as
`N + 1`. -/
def MethodCandidate.argsResolved (candidate : MethodCandidate) : Bool :=
  candidate.implArgs.all GenericArg.isConcrete

def resolvedMethodCandidates (candidates : List MethodCandidate) :
    List MethodCandidate :=
  candidates.filter MethodCandidate.argsResolved

def methodUndominatedBy (other candidate : MethodCandidate) : Bool :=
  candidate.specificity < other.specificity

def methodIsMaximalIn
    (candidates : List MethodCandidate)
    (candidate : MethodCandidate) : Bool :=
  candidates.any (fun item => item == candidate)
    && !candidates.any (fun other => methodUndominatedBy other candidate)

/-- Maximal frontier used by impl methods and trait default methods.

This abstracts `collect_specificity_maximal_impl_method_candidates_with_filter`
and `collect_specificity_maximal_trait_default_method_candidates`. Equal or
incomparable candidates both survive, so later resolution can report ambiguity. -/
def maximalMethodCandidates (candidates : List MethodCandidate) :
    List MethodCandidate :=
  candidates.filter (methodIsMaximalIn candidates)

def uniqueMaximalMethod? (candidates : List MethodCandidate) :
    Option MethodCandidate :=
  match maximalMethodCandidates (resolvedMethodCandidates candidates) with
  | [candidate] => some candidate
  | _ => none

/-- A source can be absent, uniquely resolved, or ambiguous.

The `ambiguous` case is deliberately terminal: compiler lookup must emit an
error result for the ambiguous source instead of silently trying a lower-priority
source such as a trait default. -/
inductive MethodOutcome where
  | missing : MethodOutcome
  | found : MethodCandidate -> MethodOutcome
  | ambiguous : List MethodCandidate -> MethodOutcome
  deriving DecidableEq, Repr

/-- Resolve one method source after filtering unresolved generic arguments.

An empty source is missing; one maximal candidate resolves; more than one
maximal candidate is the ambiguity reported by method lookup. -/
def resolveMethodSource (candidates : List MethodCandidate) : MethodOutcome :=
  match maximalMethodCandidates (resolvedMethodCandidates candidates) with
  | [] => .missing
  | [candidate] => .found candidate
  | ambiguous => .ambiguous ambiguous

/-- Query method sources in the same order as `resolve_named_method_in_type`.

The caller supplies sources in compiler priority order, for example:
trait-object receiver, inherent impls, active bounds, projection-assoc bounds,
ordinary impl methods, trait default methods, and invalid self-ref impl errors.
-/
def lookupBySources : List (List MethodCandidate) -> MethodOutcome
  | [] => .missing
  | source :: rest =>
      match resolveMethodSource source with
      | .missing => lookupBySources rest
      | outcome => outcome

/-- Old replacement-style method selection shape, kept as a regression oracle.

It is unsound for method lookup for the same reason it was unsound for global
associated-type projection: two equally specific where-bound impl methods would
be collapsed to the first candidate. -/
def selectMethodByReplacement? : List MethodCandidate -> Option MethodCandidate
  | [] => none
  | candidate :: rest =>
      rest.foldl
        (fun selected current =>
          match selected with
          | none => some current
          | some old =>
              if old.specificity < current.specificity then some current else selected)
        (some candidate)

theorem found_source_precedes_later_sources
    (source : List MethodCandidate)
    (later : List (List MethodCandidate))
    (candidate : MethodCandidate)
    (h : resolveMethodSource source = .found candidate) :
    lookupBySources (source :: later) = .found candidate := by
  simp [lookupBySources, h]

theorem ambiguous_source_stops_lookup
    (source : List MethodCandidate)
    (later : List (List MethodCandidate))
    (ambiguous : List MethodCandidate)
    (h : resolveMethodSource source = .ambiguous ambiguous) :
    lookupBySources (source :: later) = .ambiguous ambiguous := by
  simp [lookupBySources, h]

theorem unresolved_impl_args_hide_method
    (candidate : MethodCandidate)
    (h : candidate.argsResolved = false) :
    resolveMethodSource [candidate] = .missing := by
  cases candidate with
  | mk impl method result implArgs specificity =>
      have hArgs : implArgs.all GenericArg.isConcrete = false := by
        simpa [MethodCandidate.argsResolved] using h
      simp [resolveMethodSource, maximalMethodCandidates, resolvedMethodCandidates,
        MethodCandidate.argsResolved, hArgs]

def methodValue : MethodId := 1
def methodNext : MethodId := 2
def methodScore : MethodId := 3

def inherentNext : MethodCandidate :=
  { impl := 10
    method := methodNext
    result := tyI32
    implArgs := []
    specificity := 0 }

def traitNext : MethodCandidate :=
  { impl := 11
    method := methodNext
    result := tyFnI32
    implArgs := []
    specificity := 0 }

def blanketScore (arg : GenericArg) : MethodCandidate :=
  { impl := 20
    method := methodScore
    result := tyI32
    implArgs := [arg]
    specificity := 0 }

def constSpecificScore : MethodCandidate :=
  { impl := 21
    method := methodScore
    result := tyFnI32
    implArgs := [.const (.lit 4)]
    specificity := 1 }

def whereAScore : MethodCandidate :=
  { impl := 30
    method := methodScore
    result := tyI32
    implArgs := [.tyAtom 32]
    specificity := 0 }

def whereBScore : MethodCandidate :=
  { impl := 31
    method := methodScore
    result := tyFnI32
    implArgs := [.tyAtom 32]
    specificity := 0 }

def defaultValueI32 : MethodCandidate :=
  { impl := 40
    method := methodValue
    result := tyI32
    implArgs := [.tyAtom 32]
    specificity := 0 }

def defaultValueBool : MethodCandidate :=
  { impl := 41
    method := methodValue
    result := tyFnI32
    implArgs := [.tyAtom 1]
    specificity := 0 }

/-- Inherent methods win over same-name trait impl methods.

This is the formal counterpart of
`soundness/run-pass/coherence/inherent_method_preferred_over_trait_method.kn`. -/
example :
    lookupBySources [[inherentNext], [traitNext]] = .found inherentNext := by
  native_decide

/-- A lower-priority trait impl is not consulted after an inherent ambiguity. -/
example :
    lookupBySources [[whereAScore, whereBScore], [constSpecificScore]]
      = .ambiguous [whereAScore, whereBScore] := by
  native_decide

/-- Const-specific impl methods can dominate a blanket impl method. -/
example :
    resolveMethodSource [blanketScore (.const (.lit 4)), constSpecificScore]
      = .found constSpecificScore := by
  native_decide

/-- Open const-generic impl arguments do not expose a method. -/
example :
    resolveMethodSource [blanketScore (.const (.param 0))] = .missing := by
  native_decide

/-- Open const-generic expressions are also hidden until substitution closes them. -/
example :
    resolveMethodSource [blanketScore (.const (.add (.param 0) (.lit 1)))]
      = .missing := by
  native_decide

/-- Incomparable where-bound impl methods remain ambiguous. -/
example :
    resolveMethodSource [whereAScore, whereBScore]
      = .ambiguous [whereAScore, whereBScore] := by
  native_decide

/-- Replacement-style selection would silently pick one incomparable method. -/
example :
    selectMethodByReplacement? [whereAScore, whereBScore] = some whereAScore := by
  native_decide

/-- A concrete impl method wins before trait default methods are considered. -/
example :
    lookupBySources [[], [], [], [], [constSpecificScore], [defaultValueI32]]
      = .found constSpecificScore := by
  native_decide

/-- Equally specific trait default methods are rejected instead of picking one. -/
example :
    lookupBySources [[], [], [], [], [], [defaultValueI32, defaultValueBool]]
      = .ambiguous [defaultValueI32, defaultValueBool] := by
  native_decide

end KernFormal.MethodLookup
