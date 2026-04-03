use super::{Session, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
    Ice, // Internal Compiler Error
}

impl DiagnosticLevel {
    /// Plain-text name used in non-colored output and logs.
    pub fn name(&self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Note => "note",
            DiagnosticLevel::Ice => "ICE",
        }
    }

    /// ANSI color prefix for terminal rendering.
    pub fn color_prefix(&self) -> &'static str {
        match self {
            DiagnosticLevel::Error => "\x1b[31;1m",   // Bold red.
            DiagnosticLevel::Warning => "\x1b[33;1m", // Bold yellow.
            DiagnosticLevel::Note => "\x1b[36;1m",    // Bold cyan.
            DiagnosticLevel::Ice => "\x1b[35;1m",     // Bold magenta.
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

        // ICE diagnostics abort immediately after printing the captured context.
        if is_ice {
            self.sess.print_single_diagnostic(&self.diag);

            let (bold, reset) = if self.sess.use_color {
                ("\x1b[1m", "\x1b[0m")
            } else {
                ("", "")
            };
            eprintln!("\n{}Kern Compiler Internal Error (ICE) {}", bold, reset);
            eprintln!("This is a bug in the compiler. Please report this issue at:");
            eprintln!("{}https://github.com/softfault/kern/issues{}", bold, reset);
            eprintln!("Please include the code snippet above in your report.");

            if cfg!(test) {
                panic!("Compiler ICE: {}", self.diag.message);
            } else {
                std::process::exit(101);
            }
        }
    }
}
