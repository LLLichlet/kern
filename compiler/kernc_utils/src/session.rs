use super::{
    Diagnostic, DiagnosticBuilder, DiagnosticLevel, FileId, Interner, NodeId, SourceManager, Span,
    SymbolId,
};
use crate::config::{CompileOptions, TargetMachine};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;

pub struct Session {
    // --- 1. 基础工具 ---
    pub interner: Interner,
    pub source_manager: SourceManager,

    // --- 2. 诊断系统 ---
    pub diagnostics: Vec<Diagnostic>,
    pub error_count: usize,
    pub use_color: bool,

    // --- 3. 全局状态 ---
    pub next_node_id: u32,

    // --- 4. 编译选项  ---
    pub target: TargetMachine,
    pub custom_defines: HashMap<String, String>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            interner: Interner::new(),
            source_manager: SourceManager::new(),
            diagnostics: Vec::new(),
            error_count: 0,
            next_node_id: 0,
            use_color: std::io::stderr().is_terminal(),
            target: TargetMachine::default(),
            custom_defines: HashMap::new(),
        }
    }

    pub fn next_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        NodeId(id)
    }

    pub fn report(&mut self, span: Span, level: DiagnosticLevel, msg: String) {
        if level == DiagnosticLevel::Error || level == DiagnosticLevel::Ice {
            self.error_count += 1;
        }
        self.diagnostics.push(Diagnostic::new(level, span, msg));
    }

    pub fn emit_warning(&mut self, span: Span, msg: String) {
        self.report(span, DiagnosticLevel::Warning, msg);
    }

    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    pub fn emit_error(&mut self, span: Span, msg: impl Into<String>) {
        self.struct_error(span, msg).emit();
    }

    pub fn emit_ice(&mut self, span: Span, msg: impl Into<String>) {
        DiagnosticBuilder::new(self, DiagnosticLevel::Ice, span, msg.into())
            .with_hint("This is a bug in the Kern compiler. Please report it!")
            .emit(); // emit 内部会处理直接退出
    }

    pub fn struct_error(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        DiagnosticBuilder::new(self, DiagnosticLevel::Error, span, msg.into())
    }

    pub fn struct_warning(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        DiagnosticBuilder::new(self, DiagnosticLevel::Warning, span, msg.into())
    }

    pub fn print_diagnostics(&self) {
        for diag in &self.diagnostics {
            self.print_single_diagnostic(diag);
        }
    }

    pub fn print_single_diagnostic(&self, diag: &Diagnostic) {
        let location = self.source_manager.lookup_location(diag.primary_span);

        let (filename, line, col) = match &location {
            Some(loc) => {
                let fname = self
                    .source_manager
                    .get_file_path(loc.file_id)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                (fname, loc.line, loc.col)
            }
            None => ("<unknown>".to_string(), 0, 0),
        };

        let (bold_start, reset, prefix, color_eq) = if self.use_color {
            (
                "\x1b[1m",
                "\x1b[0m",
                diag.level.color_prefix(),
                "\x1b[36;1m",
            )
        } else {
            ("", "", "", "")
        };

        eprintln!(
            "{}{}:{}:{}: {}{}{} {}{}{}",
            bold_start,
            filename,
            line,
            col,
            prefix,
            diag.level.name(),
            reset,
            bold_start,
            diag.message,
            reset
        );

        self.print_source_snippet(diag.primary_span, diag.level);

        for (rel_span, rel_label) in &diag.related_spans {
            eprintln!(
                "   {}={} {}note:{} {}",
                color_eq, reset, bold_start, reset, rel_label
            );
            self.print_source_snippet(*rel_span, DiagnosticLevel::Note);
        }

        for hint in &diag.hints {
            eprintln!(
                "   {}={} {}help:{} {}",
                color_eq, reset, bold_start, reset, hint
            );
        }
        eprintln!();
    }

    fn print_source_snippet(&self, span: Span, level: DiagnosticLevel) {
        if let Some(loc) = self.source_manager.lookup_location(span) {
            if let Some(line_text) = self.source_manager.get_line_text(loc.clone()) {
                let line_num_str = format!("{}", loc.line);
                let padding = " ".repeat(line_num_str.len());

                eprintln!(" {} |", padding);
                eprintln!(" {} | {}", line_num_str, line_text.trim_end());
                eprint!(" {} | ", padding);

                // 安全截断：波浪线长度不能超过当前行的物理剩余长度
                let text_len = line_text.trim_end().len();
                let col_offset = loc.col.saturating_sub(1);
                let max_possible_carets = text_len.saturating_sub(col_offset);

                let raw_span_len = span.end.saturating_sub(span.start);
                let print_len = std::cmp::max(1, std::cmp::min(raw_span_len, max_possible_carets));

                let carets = "^".repeat(print_len);

                if self.use_color {
                    eprintln!(
                        "{}{}{}\x1b[0m",
                        " ".repeat(col_offset),
                        level.color_prefix(),
                        carets
                    );
                } else {
                    let indent_str: String = line_text
                        .chars()
                        .take(col_offset)
                        .map(|c| if c == '\t' { '\t' } else { ' ' })
                        .collect();
                    eprintln!("{}{}{}\x1b[0m", indent_str, level.color_prefix(), carets);
                }
            }
        }
    }

    pub fn intern(&mut self, string: &str) -> SymbolId {
        self.interner.intern(string)
    }

    pub fn resolve(&self, sym: SymbolId) -> &str {
        self.interner.resolve(sym).unwrap_or("<unknown>")
    }

    pub fn load_file<P: AsRef<Path>>(&mut self, path: P) -> std::io::Result<FileId> {
        self.source_manager.load_file(path)
    }

    pub fn apply_options(&mut self, options: &CompileOptions) {
        self.target = options.target.clone();
        self.custom_defines = options.custom_defines.clone();
    }
}
