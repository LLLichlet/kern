#![allow(unused)]
use super::Span;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// =========================================================
/// 基础类型定义
/// =========================================================

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub usize);

impl FileId {
    pub const fn get(&self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub file_id: FileId,
    pub line: usize, // 1-based 行号
    pub col: usize,  // 1-based 列号
}

/// =========================================================
/// SourceFile: 单个文件的底层管理者
/// =========================================================

#[derive(Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub name: String,
    pub src: String,
    pub line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(path: PathBuf, src: String) -> Self {
        let line_starts = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(i, _)| i + 1))
            .collect();

        let name = path.to_string_lossy().to_string();

        Self {
            path,
            name,
            src,
            line_starts,
        }
    }

    /// 核心算法：二分查找行号 (1-based)
    pub fn lookup_line(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    /// 计算 (Line, Col)，不涉及 Location 结构体，纯数学运算
    pub fn lookup_line_col(&self, offset: usize) -> (usize, usize) {
        let line = self.lookup_line(offset);
        let line_start = self.line_starts[line - 1];
        let col = offset - line_start + 1;
        (line, col)
    }

    /// 获取指定行的文本内容 (用于报错显示)
    /// 这里的 line 是 1-based
    pub fn get_line_text(&self, line: usize) -> Option<&str> {
        if line == 0 || line > self.line_starts.len() {
            return None;
        }

        let line_idx = line - 1; // 转为 0-based 索引
        let start = self.line_starts[line_idx];

        let end = if line_idx + 1 < self.line_starts.len() {
            self.line_starts[line_idx + 1] - 1 // -1 是为了去掉换行符
        } else {
            self.src.len()
        };

        // 防御性切片
        if start <= end && end <= self.src.len() {
            Some(&self.src[start..end])
        } else {
            Some("") // 处理空行或异常情况
        }
    }

    /// [LSP 必需] 将 (行号, 列号) 转换为字节偏移量
    pub fn offset_at(&self, line: usize, col: usize) -> Option<usize> {
        // line 必须是 1-based ? 通常 LSP 是 0-based，这里假设你使用内部统一的 1-based
        // 如果这里 line 是 0-based，请去掉下面的 -1
        if line == 0 || line > self.line_starts.len() {
            return None;
        }

        let line_idx = line - 1;
        let start = self.line_starts[line_idx];

        // 计算当前行结束位置（包含换行符）
        let end_limit = if line_idx + 1 < self.line_starts.len() {
            self.line_starts[line_idx + 1]
        } else {
            self.src.len() + 1 // +1 允许指向 EOF
        };
        let target = start + col;
        if target >= end_limit {
            return None;
        }

        Some(target)
    }
}

/// =========================================================
/// SourceManager: 全局资源中心
/// =========================================================

#[derive(Debug, Default)]
pub struct SourceManager {
    files: Vec<SourceFile>,
}

impl SourceManager {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// 加载文件（带去重逻辑）
    pub fn load_file<P: AsRef<Path>>(&mut self, path: P) -> io::Result<FileId> {
        let path = path.as_ref();
        let abs_path = fs::canonicalize(path)?;

        // 简单查重 O(N)，文件多时可优化为 HashMap
        if let Some((id, _)) = self
            .files
            .iter()
            .enumerate()
            .find(|(_, f)| f.path == abs_path)
        {
            return Ok(FileId(id));
        }

        let src = fs::read_to_string(&abs_path)?;
        let file = SourceFile::new(abs_path, src);
        let id = FileId(self.files.len());
        self.files.push(file);
        Ok(id)
    }

    /// 添加虚拟文件（用于测试或 REPL）
    pub fn add_file(&mut self, name: String, src: String) -> FileId {
        let file = SourceFile::new(PathBuf::from(name), src);
        let id = FileId(self.files.len());
        self.files.push(file);
        id
    }

    /// 核心查询 1: 将 Span 转换为坐标 (Location)
    /// 返回纯数据，无生命周期
    pub fn lookup_location(&self, span: Span) -> Option<Location> {
        let file = self.files.get(span.file.get())?;
        let (line, col) = file.lookup_line_col(span.start);

        Some(Location {
            file_id: span.file,
            line,
            col,
        })
    }

    /// 核心查询 2: 获取 Span 对应的源码切片
    /// 需要生命周期，因为它返回的是 self 中字符串的借用
    pub fn slice_source(&self, span: Span) -> &str {
        if let Some(file) = self.files.get(span.file.get()) {
            // 简单的边界检查
            if span.start <= span.end && span.end <= file.src.len() {
                return &file.src[span.start..span.end];
            }
        }
        ""
    }

    /// 核心查询 3: 获取 Location 所在行的完整文本
    /// 用于错误报告
    pub fn get_line_text(&self, location: Location) -> Option<&str> {
        let file = self.files.get(location.file_id.get())?;
        file.get_line_text(location.line)
    }

    // --- 辅助方法 ---

    pub fn get_file(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.get())
    }

    pub fn get_file_name(&self, id: FileId) -> Option<&str> {
        self.files.get(id.get()).map(|f| f.name.as_str())
    }

    pub fn get_file_path(&self, id: FileId) -> Option<&PathBuf> {
        self.files.get(id.get()).map(|f| &f.path)
    }

    pub fn update_file(&mut self, id: FileId, new_src: String) {
        if let Some(file) = self.files.get_mut(id.get()) {
            // 直接覆盖旧文件，保持 path 不变
            *file = SourceFile::new(file.path.clone(), new_src);
        }
    }
}
