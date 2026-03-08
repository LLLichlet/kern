#![allow(unused)]
use crate::diagnostic::{Diagnostic, DiagnosticLevel};
use crate::utils::*;
use crate::sema::*;
use crate::config::TargetMachine;

use std::collections::HashMap;

pub struct Context {
    pub interner: Interner,
    pub source_manager: SourceManager,
    pub diagnostics: Vec<Diagnostic>,
    pub error_count: usize,
    pub type_registry: ty::TypeRegistry,
    pub defs: Vec<def::Def>,
    pub scopes: scope::SymbolTable,
    pub node_types: HashMap<crate::ast::NodeId, ty::TypeId>,
    pub target: TargetMachine,
    pub next_node_id: u32,
}

impl Context {
    pub fn new() -> Self {
        Self {
            interner: Interner::new(),
            source_manager: SourceManager::new(),
            diagnostics: Vec::new(),
            error_count: 0,
            type_registry: ty::TypeRegistry::new(),
            defs: Vec::new(),
            scopes: scope::SymbolTable::new(),
            node_types: HashMap::new(),
            target: TargetMachine::default(),
            next_node_id: 0,
        }
    }

    pub fn next_node_id(&mut self) -> crate::ast::NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        crate::ast::NodeId(id)
    }

    /// 核心方法：报告诊断信息
    pub fn report(&mut self, span: Span, level: DiagnosticLevel, msg: String) {
        if level == DiagnosticLevel::Error {
            self.error_count += 1;
        }

        self.diagnostics.push(Diagnostic::new(level, span, msg));
    }

    /// 快捷方法：报告 Error
    /// Rust 习惯：调用者使用 format!("...", args) 生成 msg
    pub fn emit_error(&mut self, span: Span, msg: String) {
        self.report(span, DiagnosticLevel::Error, msg);
    }
    
    /// 快捷方法：报告 Warning
    pub fn emit_warning(&mut self, span: Span, msg: String) {
        self.report(span, DiagnosticLevel::Warning, msg);
    }

    /// 判断是否有致命错误
    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    /// 字符串驻留 (Interning)
    pub fn intern(&mut self, string: &str) -> SymbolId {
        self.interner.intern(string)
    }

    /// 解析驻留的字符串
    pub fn resolve(&self, sym: SymbolId) -> &str {
        // 假设 interner.resolve 返回 Option<&str>，这里为了方便直接 unwrap 
        // 或者返回 "<unknown>"，视 interner 实现而定
        self.interner.resolve(sym).unwrap_or("<unknown>")
    }
    
    /// 加载文件 (代理 SourceManager)
    pub fn load_file<P: AsRef<std::path::Path>>(&mut self, path: P) -> std::io::Result<FileId> {
        self.source_manager.load_file(path)
    }

    /// 打印所有诊断信息到 stderr
    pub fn print_diagnostics(&self) {
        for diag in &self.diagnostics {
            self.print_diagnostic(diag);
        }
    }

    fn print_diagnostic(&self, diag: &Diagnostic) {
        // 获取位置信息 (Line, Col)
        // 这里的 lookup_location 来自之前的 SourceManager 实现
        let location = self.source_manager.lookup_location(diag.span);

        // 获取文件名
        let filename = if let Some(loc) = location {
             self.source_manager
                .get_file_path(loc.file_id)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string())
        } else {
            "<unknown>".to_string()
        };

        // 获取行号和列号
        let (line, col) = if let Some(loc) = location {
            (loc.line, loc.col)
        } else {
            (0, 0)
        };
        
        // 打印 Header: "main.rs:10:5: Error: message"
        eprintln!(
            "{}:{}:{}: {}: {}",
            filename,
            line,
            col,
            diag.level,
            diag.message
        );

        // 进阶：打印源代码片段并高亮
        // 如果 SourceManager 提供了 slice_source 或 get_line_text
        if let Some(loc) = location {
            if let Some(line_text) = self.source_manager.get_line_text(loc) {
                // 打印源代码行
                eprintln!("   | ");
                eprintln!(" {} | {}", line, line_text.trim_end());
                
                // 打印下划线指针 (简单的 ^ 指示)
                // 注意：这里需要处理 Tab 和宽字符，简单起见先打印空格
                eprint!("   | ");
                for _ in 0..col {
                    eprint!(" "); // 实际上应该匹配 line_text 前面的空白字符宽度
                }
                eprintln!("^");
            }
        }
        eprintln!(); // 空行分隔
    }

    pub fn add_def(&mut self, def: def::Def) -> ty::DefId {
        let id = ty::DefId(self.defs.len() as u32);
        self.defs.push(def);
        id
    }
}

// 允许默认初始化
impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_error_reporting() {
        let mut ctx = Context::new();
        
        // 模拟一个虚拟文件
        let src = "let x = ;";
        let file_id = ctx.source_manager.add_file("test.rs".to_string(), src.to_string());
        
        // 模拟发现错误: let x = ^; (缺少表达式)
        // Span 假设在 '=' 后面，位置 8
        let span = Span { file: file_id, start: 8, end: 9 };
        
        // 报告错误
        // Rust 的 format! 宏是在调用处使用的，非常灵活
        ctx.emit_error(span, format!("Expected expression, found {:?}", ";"));

        assert_eq!(ctx.error_count, 1);
        assert!(ctx.has_errors());
        
        // 打印看看 (在 `cargo test` 中需要 --nocapture 才能看到输出)
        ctx.print_diagnostics();
    }
    
    #[test]
    fn test_interning() {
        let mut ctx = Context::new();
        let sym1 = ctx.intern("hello");
        let sym2 = ctx.intern("hello");
        let sym3 = ctx.intern("world");
        
        assert_eq!(sym1, sym2);
        assert_ne!(sym1, sym3);
        
        assert_eq!(ctx.resolve(sym1), "hello");
    }
}