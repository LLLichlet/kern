//! Default compiler options used by the LSP analysis engine.

use kernc_utils::config::{CompileOptions, LibraryBundle};

pub(crate) fn default_analysis_compile_options() -> CompileOptions {
    CompileOptions {
        library_bundle: LibraryBundle::Std,
        ..CompileOptions::default()
    }
}
