import KernFormal.Coherence
import KernFormal.Mono

/-!
Formal model for kmeta source-snapshot imports.

Kern's current `kmeta` format does not serialize semantic `TypeId`s.  It stores
a source snapshot plus a manifest.  Importing a package reparses that snapshot
with `is_imported = true`, assigns the manifest package/root identity, and then
reruns collection/type resolution in the consuming compiler session.

The soundness-critical invariant is therefore source-preservation: imported
trait/impl heads must keep their type args, const args, assoc bindings, and
package locality when they are reconstructed.  If const args were erased during
this path, coherence, proof search, monomorphization, and vtable identity would
all see the wrong program.
-/

namespace KernFormal.ImportMetadata

open KernFormal.FakeRefl
open KernFormal.Coherence
open KernFormal.Mono

abbrev PackageId := Nat

structure SourceImplHead where
  package : PackageId
  imported : Bool
  target : List HeadPat
  traitId : TraitId
  traitArgs : List HeadPat
  assocBindings : List (AssocId × HeadPat)
  deriving DecidableEq, Repr

structure ImportedImplHead where
  package : PackageId
  imported : Bool
  target : List HeadPat
  traitId : TraitId
  traitArgs : List HeadPat
  assocBindings : List (AssocId × HeadPat)
  deriving DecidableEq, Repr

inductive Linkage where
  | internal
  | external
  deriving DecidableEq, Repr

structure FunctionItem where
  imported : Bool
  hasBody : Bool
  isTraitImplMethod : Bool
  linkage : Linkage
  deriving DecidableEq, Repr

instance : BEq SourceImplHead where
  beq left right :=
    left.package == right.package
      && left.imported == right.imported
      && left.target == right.target
      && left.traitId == right.traitId
      && left.traitArgs == right.traitArgs
      && left.assocBindings == right.assocBindings

instance : BEq ImportedImplHead where
  beq left right :=
    left.package == right.package
      && left.imported == right.imported
      && left.target == right.target
      && left.traitId == right.traitId
      && left.traitArgs == right.traitArgs
      && left.assocBindings == right.assocBindings

/-- Reconstruct an imported impl from a source snapshot.

This mirrors loader + collection behavior for kmeta packages: the source shape
is preserved, while the module is marked as imported in the consuming session. -/
def importSourceImpl (head : SourceImplHead) : ImportedImplHead :=
  { package := head.package
    imported := true
    target := head.target
    traitId := head.traitId
    traitArgs := head.traitArgs
    assocBindings := head.assocBindings }

def importedToCoherenceHead (head : ImportedImplHead) : ImplHead :=
  { target := head.target
    trait := head.traitArgs
    whereCount := 0 }

def traitLookupKey (package : PackageId) (traitId : TraitId) : PackageId × TraitId :=
  (package, traitId)

def importedTraitLookupKey (head : ImportedImplHead) : PackageId × TraitId :=
  traitLookupKey head.package head.traitId

def monoKeyFromImportedFunction
    (defn : DefId)
    (args : List GenericArg) : MonoKey :=
  { defn, args }

def traitRefFromImported (head : ImportedImplHead) : TraitRef :=
  { trait := head.traitId
    args := head.traitArgs.map (fun pat =>
      match pat with
      | .tyAtom id => .tyAtom id
      | .tyParam id => .tyParam id
      | .constLit value => .const (.lit value)
      | .constParam id => .const (.param id))
    assocBindings := [] }

/- Imported functions with no body are declarations in the consuming module.
They must be external even when the original source item was private, otherwise
the backend would produce an invalid or unresolvable private declaration. -/
def importedDeclarationLinkage (item : FunctionItem) : Linkage :=
  if item.imported && !item.hasBody then .external else item.linkage

/- Publishing kmeta exposes source snapshots whose trait impl methods may be
referenced later from a consumer-built vtable adapter.  Those methods must be
materialized and linkable in the producer object even when ordinary reachability
inside the producer package does not call them directly. -/
def metadataPublishesFunction (publishingMetadata : Bool) (item : FunctionItem) : Bool :=
  publishingMetadata && !item.imported && item.isTraitImplMethod

def producerImplMethod : FunctionItem :=
  { imported := false
    hasBody := true
    isTraitImplMethod := true
    linkage := .internal }

def importedImplMethodDecl : FunctionItem :=
  { imported := true
    hasBody := false
    isTraitImplMethod := true
    linkage := .internal }

def sourceMarker1 : SourceImplHead :=
  { package := 1
    imported := false
    target := [.tyAtom 10]
    traitId := 77
    traitArgs := [.constLit 1]
    assocBindings := [] }

def sourceMarker2 : SourceImplHead :=
  { sourceMarker1 with traitArgs := [.constLit 2] }

def sourceBox4Assoc : SourceImplHead :=
  { package := 1
    imported := false
    target := [.tyAtom 20, .constLit 4]
    traitId := 88
    traitArgs := [.constLit 4]
    assocBindings := [(5, .tyAtom 32)] }

def importedMarker1 : ImportedImplHead := importSourceImpl sourceMarker1
def importedMarker2 : ImportedImplHead := importSourceImpl sourceMarker2
def importedBox4Assoc : ImportedImplHead := importSourceImpl sourceBox4Assoc

theorem import_marks_impl_imported (head : SourceImplHead) :
    (importSourceImpl head).imported = true := by
  simp [importSourceImpl]

theorem import_preserves_trait_args (head : SourceImplHead) :
    (importSourceImpl head).traitArgs = head.traitArgs := by
  simp [importSourceImpl]

theorem import_preserves_assoc_bindings (head : SourceImplHead) :
    (importSourceImpl head).assocBindings = head.assocBindings := by
  simp [importSourceImpl]

theorem import_preserves_package_trait_lookup_key (head : SourceImplHead) :
    importedTraitLookupKey (importSourceImpl head) = traitLookupKey head.package head.traitId := by
  simp [importedTraitLookupKey, traitLookupKey, importSourceImpl]

/-- Imported const trait args remain disjoint after reconstruction. -/
example :
    headsOverlap
      (importedToCoherenceHead importedMarker1)
      (importedToCoherenceHead importedMarker2) = false := by
  native_decide

/-- Imported assoc bindings keep their const-specialized head identity. -/
example :
    importedBox4Assoc.assocBindings = [(5, .tyAtom 32)] := by
  native_decide

/-- Trait lookup grouping includes imported package identity. -/
example :
    importedTraitLookupKey importedMarker1 = (1, 77) := by
  native_decide

/-- Imported source snapshots do not erase const args before mono key creation. -/
example :
    monoKeyFromImportedFunction 500 [.const (.lit 4)]
      != monoKeyFromImportedFunction 500 [.const (.lit 5)] := by
  native_decide

/-- Imported trait refs retain const args for vtable/proof identity. -/
example :
    (traitRefFromImported importedMarker1).args = [.const (.lit 1)] := by
  native_decide

/-- kmeta producers must materialize otherwise-unreached trait impl methods. -/
example :
    metadataPublishesFunction true producerImplMethod = true := by
  native_decide

/-- kmeta consumers must not declare imported no-body methods as internal. -/
example :
    importedDeclarationLinkage importedImplMethodDecl = .external := by
  native_decide

/-- Imported orphan checking treats a foreign trait as foreign to the consumer package. -/
example :
    orphanLegal 2 (.foreign importedMarker1.package) (.primitive 32) = false := by
  native_decide

/-- A consumer package may implement an imported trait for a pointer to its local type. -/
example :
    orphanLegal 2 (.foreign importedMarker1.package) (.ptr (.localDef 2)) = true := by
  native_decide

end KernFormal.ImportMetadata
