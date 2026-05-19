//! File-relative byte spans.
//!
//! Spans are half-open byte ranges `[start, end)` inside a `SourceFile`.  Line
//! and column coordinates are derived lazily by `SourceManager` so the core AST
//! can stay compact and cheap to clone.

use super::source::FileId;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub file: FileId,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn to(self, other: Span) -> Self {
        // Merging spans from different files would make diagnostics point at a
        // nonsensical byte range, so fail loudly during compiler development.
        assert_eq!(
            self.file, other.file,
            "Cannot merge spans from different files!"
        );

        Self {
            file: self.file,
            start: std::cmp::min(self.start, other.start),
            end: std::cmp::max(self.end, other.end),
        }
    }
}
