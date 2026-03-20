use super::source::FileId;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub file: FileId,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn to(self, other: Span) -> Self {
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
