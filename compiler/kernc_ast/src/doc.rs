use kernc_utils::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLine {
    pub span: Span,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocBlock {
    pub span: Span,
    pub lines: Vec<DocLine>,
}
