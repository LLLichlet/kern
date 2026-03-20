use super::{Session, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
    Ice, // Internal Compiler Error
}

impl DiagnosticLevel {
    /// 纯文本名称，用于日志重定向
    pub fn name(&self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Note => "note",
            DiagnosticLevel::Ice => "ICE",
        }
    }

    /// ANSI 颜色控制码前缀
    pub fn color_prefix(&self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "\x1b[31;1m",   // 红色粗体
            DiagnosticLevel::Warning => "\x1b[33;1m", // 黄色粗体
            DiagnosticLevel::Note => "\x1b[36;1m",    // 青色粗体
            DiagnosticLevel::Ice => "\x1b[35;1m",     // 紫色粗体
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub primary_span: Span,
    pub message: String,
    pub hints: Vec<String>,
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

#[must_use = "Diagnostics must be emitted to take effect"]
pub struct DiagnosticBuilder<'a> {
    sess: &'a mut Session,
    diag: Diagnostic,
}

impl<'a> DiagnosticBuilder<'a> {
    pub fn new(sess: &'a mut Session, level: DiagnosticLevel, span: Span, msg: String) -> Self {
        Self {
            sess,
            diag: Diagnostic::new(level, span, msg),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.diag = self.diag.with_hint(hint);
        self
    }

    pub fn with_span_label(mut self, span: Span, label: impl Into<String>) -> Self {
        self.diag.related_spans.push((span, label.into()));
        self
    }

    pub fn emit(self) {
        let is_ice = self.diag.level == DiagnosticLevel::Ice;
        if self.diag.level == DiagnosticLevel::Error || is_ice {
            self.sess.error_count += 1;
        }
        self.sess.diagnostics.push(self.diag.clone());

        // 遇到编译器崩溃，打印现场后立即熔断
        if is_ice {
            self.sess.print_single_diagnostic(&self.diag);
            if cfg!(test) {
                panic!("Compiler ICE: {}", self.diag.message);
            } else {
                eprintln!("\nCompiler panicked due to an internal error. Aborting.");
                std::process::exit(101);
            }
        }
    }
}
