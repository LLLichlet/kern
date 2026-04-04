use super::Span;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// =========================================================
/// Basic source location types
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
    pub line: usize, // 1-based line number.
    pub col: usize,  // 1-based column number.
}

/// =========================================================
/// SourceFile: backing storage for one loaded source file
/// =========================================================

#[derive(Debug, Clone)]
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

    /// Map a byte offset to a 1-based line number via binary search.
    pub fn lookup_line(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    /// Compute the 1-based line and column for a byte offset.
    pub fn lookup_line_col(&self, offset: usize) -> (usize, usize) {
        let line = self.lookup_line(offset);
        let line_start = self.line_starts[line - 1];
        let col = offset - line_start + 1;
        (line, col)
    }

    /// Return the full text of a 1-based line for diagnostics.
    pub fn get_line_text(&self, line: usize) -> Option<&str> {
        if line == 0 || line > self.line_starts.len() {
            return None;
        }

        let line_idx = line - 1; // Convert to a 0-based index.
        let start = self.line_starts[line_idx];

        let end = if line_idx + 1 < self.line_starts.len() {
            self.line_starts[line_idx + 1] - 1 // Trim the trailing newline.
        } else {
            self.src.len()
        };

        // Guard against inconsistent spans in recovery paths.
        if start <= end && end <= self.src.len() {
            Some(&self.src[start..end])
        } else {
            Some("") // Treat invalid slices as an empty line.
        }
    }

    /// Convert a 1-based line/column pair back into a byte offset.
    pub fn offset_at(&self, line: usize, col: usize) -> Option<usize> {
        // The compiler uses 1-based coordinates internally.
        if line == 0 || line > self.line_starts.len() {
            return None;
        }

        let line_idx = line - 1;
        let start = self.line_starts[line_idx];

        // Compute the exclusive end of the current line, including the newline if present.
        let end_limit = if line_idx + 1 < self.line_starts.len() {
            self.line_starts[line_idx + 1]
        } else {
            self.src.len() + 1 // Allow callers to point at EOF.
        };
        let target = start + col;
        if target >= end_limit {
            return None;
        }

        Some(target)
    }
}

/// =========================================================
/// SourceManager: global registry for loaded source files
/// =========================================================

#[derive(Debug, Default, Clone)]
pub struct SourceManager {
    files: Vec<SourceFile>,
}

impl SourceManager {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Load a file, deduplicating identical canonical paths.
    pub fn load_file<P: AsRef<Path>>(&mut self, path: P) -> io::Result<FileId> {
        let path = path.as_ref();
        let abs_path = fs::canonicalize(path)?;

        // Linear deduplication is sufficient for now; upgrade to a map if needed.
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

    /// Register an in-memory file, typically for tests or REPL usage.
    pub fn add_file(&mut self, name: String, src: String) -> FileId {
        let file = SourceFile::new(PathBuf::from(name), src);
        let id = FileId(self.files.len());
        self.files.push(file);
        id
    }

    /// Query 1: convert a span into a concrete source location.
    pub fn lookup_location(&self, span: Span) -> Option<Location> {
        let file = self.files.get(span.file.get())?;
        let (line, col) = file.lookup_line_col(span.start);

        Some(Location {
            file_id: span.file,
            line,
            col,
        })
    }

    /// Query 2: borrow the source text covered by a span.
    pub fn slice_source(&self, span: Span) -> &str {
        if let Some(file) = self.files.get(span.file.get()) {
            // Keep recovery code from slicing beyond file bounds.
            if span.start <= span.end && span.end <= file.src.len() {
                return &file.src[span.start..span.end];
            }
        }
        ""
    }

    /// Query 3: fetch the full line text that contains a location.
    pub fn get_line_text(&self, location: Location) -> Option<&str> {
        let file = self.files.get(location.file_id.get())?;
        file.get_line_text(location.line)
    }

    // --- Helpers ---

    pub fn get_file(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.get())
    }

    pub fn get_file_name(&self, id: FileId) -> Option<&str> {
        self.files.get(id.get()).map(|f| f.name.as_str())
    }

    pub fn get_file_path(&self, id: FileId) -> Option<&PathBuf> {
        self.files.get(id.get()).map(|f| &f.path)
    }

    pub fn find_file_id_by_path(&self, path: &Path) -> Option<FileId> {
        self.files
            .iter()
            .position(|file| file.path == path)
            .map(FileId)
    }

    pub fn update_file(&mut self, id: FileId, new_src: String) {
        if let Some(file) = self.files.get_mut(id.get()) {
            // Replace the contents while preserving the logical path.
            *file = SourceFile::new(file.path.clone(), new_src);
        }
    }
}
