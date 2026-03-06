#![allow(unused)]
use crate::utils::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
}

impl std::fmt::Display for DiagnosticLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticLevel::Error => write!(f, "Error"),
            DiagnosticLevel::Warning => write!(f, "Warning"),
            DiagnosticLevel::Note => write!(f, "Note"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub span: Span,
    pub message: String,
    // pub related: Vec<Diagnostic>,
}

impl Diagnostic {
    pub fn new(level: DiagnosticLevel, span: Span, message: String) -> Self {
        Self {
            level,
            span,
            message,
        }
    }
}