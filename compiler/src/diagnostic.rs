use crate::utils::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
    Ice, // Internal Compiler Error (编译器内部错误)
}

impl DiagnosticLevel {
    /// 获取带颜色的标签名称 (使用简单的 ANSI 转义码)
    pub fn color_name(&self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "\x1b[31;1merror\x1b[0m",   // 红色粗体
            DiagnosticLevel::Warning => "\x1b[33;1mwarning\x1b[0m", // 黄色粗体
            DiagnosticLevel::Note => "\x1b[36;1mnote\x1b[0m",      // 青色粗体
            DiagnosticLevel::Ice => "\x1b[35;1mICE\x1b[0m",        // 紫色粗体
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub primary_span: Span,
    pub message: String,
    pub hints: Vec<String>, // 帮助信息，比如 "help: consider adding `mut`"
    pub related_spans: Vec<(Span, String)>,
}

impl Diagnostic {
    pub fn new(level: DiagnosticLevel, span: Span, message: impl Into<String>) -> Self {
        Self {
            level,
            primary_span: span,
            message: message.into(),
            hints: Vec::new(),
            related_spans: Vec::new(),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }
}