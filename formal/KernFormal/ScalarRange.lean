/-!
Formal model for scalar match range coverage.

The compiler handles bool and integer match coverage with a separate interval
state instead of the constructor matrix used for ADTs. Source values and ranges
are lowered to closed intervals, clipped to the target type's finite domain,
then inserted into a sorted disjoint list that merges overlapping or adjacent
intervals. A later pattern is unreachable when its intervals are empty or
already covered; a match is exhaustive when the covered list is exactly the
whole domain.
-/

namespace KernFormal.ScalarRange

structure Interval where
  start : Int
  stop : Int
  deriving Repr, BEq

structure Domain where
  min : Int
  max : Int
  deriving Repr, BEq

structure Coverage where
  domain : Domain
  covered : List Interval
  deriving Repr, BEq

inductive RangeSyntax where
  /-- `start..=end`, represented internally as the closed interval `[start, end]`. -/
  | inclusive
  /-- `start...end`, represented internally as `[start, end - 1]`. -/
  | exclusive
  deriving Repr, BEq

def i8Domain : Domain := { min := -128, max := 127 }
def u8Domain : Domain := { min := 0, max := 255 }
def boolDomain : Domain := { min := 0, max := 1 }
def u128Max : Int := 340282366920938463463374607431768211455
def u128Domain : Domain := { min := 0, max := u128Max }

def emptyCoverage (domain : Domain) : Coverage :=
  { domain, covered := [] }

def valid (interval : Interval) : Bool :=
  interval.start <= interval.stop

def clip (domain : Domain) (interval : Interval) : Option Interval :=
  let clipped :=
    { start := max domain.min interval.start
      stop := min domain.max interval.stop }
  if valid clipped then some clipped else none

def rangeInterval (domain : Domain) (kind : RangeSyntax) (start stop : Int) : Option Interval :=
  let closedStop :=
    match kind with
    | .inclusive => stop
    | .exclusive => stop - 1
  clip domain { start, stop := closedStop }

def pointInterval (domain : Domain) (point : Int) : Option Interval :=
  clip domain { start := point, stop := point }

def overlapsOrAdjacent (left right : Interval) : Bool :=
  ¬ (left.stop + 1 < right.start || right.stop + 1 < left.start)

def merge (left right : Interval) : Interval :=
  { start := min left.start right.start
    stop := max left.stop right.stop }

partial def insertIntervalSorted (next : Interval) : List Interval -> List Interval
  | [] => if valid next then [next] else []
  | current :: rest =>
      if !valid next then
        current :: rest
      else if next.stop + 1 < current.start then
        next :: current :: rest
      else if current.stop + 1 < next.start then
        current :: insertIntervalSorted next rest
      else
        insertIntervalSorted (merge next current) rest

def addInterval (coverage : Coverage) (interval : Interval) : Coverage :=
  { coverage with covered := insertIntervalSorted interval coverage.covered }

def addIntervals (coverage : Coverage) (intervals : List Interval) : Coverage :=
  intervals.foldl addInterval coverage

def intervalCoveredBy (seen interval : Interval) : Bool :=
  seen.start <= interval.start && interval.stop <= seen.stop

def coversAll (coverage : Coverage) (intervals : List Interval) : Bool :=
  intervals.all (fun interval =>
    coverage.covered.any (fun seen => intervalCoveredBy seen interval))

def isFull (coverage : Coverage) : Bool :=
  match coverage.covered with
  | [interval] => interval.start == coverage.domain.min && interval.stop == coverage.domain.max
  | _ => false

def intervalEmpty? (interval : Option Interval) : Bool :=
  interval.isNone

def usefulInterval (coverage : Coverage) (interval : Option Interval) : Bool :=
  match interval with
  | none => false
  | some interval => !coversAll coverage [interval]

partial def firstUncoveredFrom (cursor max : Int) : List Interval -> Option Int
  | [] => if cursor <= max then some cursor else none
  | interval :: rest =>
      if cursor < interval.start then
        some cursor
      else
        firstUncoveredFrom (interval.stop + 1) max rest

def firstUncovered (coverage : Coverage) : Option Int :=
  firstUncoveredFrom coverage.domain.min coverage.domain.max coverage.covered

def addRange? (kind : RangeSyntax) (start stop : Int) (coverage : Coverage) : Coverage :=
  match rangeInterval coverage.domain kind start stop with
  | some interval =>
      if coversAll coverage [interval] then coverage else addInterval coverage interval
  | none => coverage

def addPoint? (point : Int) (coverage : Coverage) : Coverage :=
  match pointInterval coverage.domain point with
  | some interval =>
      if coversAll coverage [interval] then coverage else addInterval coverage interval
  | none => coverage

/-- Adjacent ranges are merged, matching `insert_*_interval`. -/
example :
    ((emptyCoverage u8Domain
      |> addRange? .inclusive 0 127
      |> addRange? .inclusive 128 255).covered ==
      [{ start := 0, stop := 255 }]) = true := by
  native_decide

/-- Covering the entire finite domain makes scalar coverage exhaustive. -/
example :
    isFull
      (emptyCoverage u8Domain
        |> addRange? .inclusive 0 127
        |> addRange? .inclusive 128 255) = true := by
  native_decide

/-- Missing zero produces the same first uncovered witness as the compiler. -/
example :
    firstUncovered (emptyCoverage u8Domain |> addRange? .inclusive 1 255) = some 0 := by
  native_decide

/-- Exclusive `...MAX` leaves `MAX` uncovered. -/
example :
    firstUncovered (emptyCoverage u128Domain |> addRange? .exclusive 0 u128Max) = some u128Max := by
  native_decide

/-- Inclusive `..=MAX` covers the full u128 domain. -/
example :
    isFull (emptyCoverage u128Domain |> addRange? .inclusive 0 u128Max) = true := by
  native_decide

/-- Signed ranges are clipped to the target type domain. -/
example :
    (rangeInterval i8Domain .inclusive (-200) (-120) ==
      some { start := -128, stop := -120 }) = true := by
  native_decide

/-- Empty ranges are useless and treated as unreachable scalar patterns. -/
example :
    usefulInterval (emptyCoverage u8Domain) (rangeInterval u8Domain .exclusive 0 0) = false := by
  native_decide

/-- A fully shadowed subrange is not useful. -/
example :
    let coverage := emptyCoverage u8Domain |> addRange? .inclusive 0 10
    usefulInterval coverage (rangeInterval u8Domain .inclusive 3 5) = false := by
  native_decide

/-- A partially new adjacent range remains useful and is merged after insertion. -/
example :
    let coverage := emptyCoverage u8Domain |> addRange? .inclusive 0 10
    usefulInterval coverage (rangeInterval u8Domain .inclusive 10 12) = true := by
  native_decide

/-- Bool coverage is the unsigned scalar domain `0..=1`. -/
example :
    isFull
      (emptyCoverage boolDomain
        |> addPoint? 0
        |> addPoint? 1) = true := by
  native_decide

end KernFormal.ScalarRange
