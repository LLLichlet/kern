mod analysis;
mod completion;
mod link;
mod signature;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kernc_ast as ast;
use kernc_codegen::{CodeGenerator, Context, InlineAsmDialect};
use kernc_lower::Lowerer;
use kernc_sema::def::DefId;
use kernc_sema::scope::ScopeId;
use kernc_sema::ty::TypeId;
use kernc_sema::{SemaContext, SemaStructureSnapshot};
use kernc_utils::Session;
use kernc_utils::SymbolId;
use kernc_utils::config::{AsmDialect, CompileOptions, DriverMode};

use crate::metadata;

pub type SourceOverrides = HashMap<PathBuf, String>;

pub struct AnalysisReport {
    pub session: Session,
    pub succeeded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisSpanReplacement {
    pub clean: kernc_utils::Span,
    pub dirty: kernc_utils::Span,
}

pub struct TargetedAnalysisReport {
    pub report: AnalysisReport,
    pub replaced_spans: Vec<AnalysisSpanReplacement>,
}

#[derive(Debug, Clone)]
pub struct AnalysisReference {
    pub reference_span: kernc_utils::Span,
    pub definition_span: kernc_utils::Span,
}

#[derive(Debug, Clone)]
pub struct AnalysisHover {
    pub span: kernc_utils::Span,
    pub contents: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisSemanticRole {
    Definition,
    Reference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisSemanticKind {
    Module,
    Namespace,
    Struct,
    Enum,
    Interface,
    Type,
    TypeParameter,
    Property,
    Variable,
    Parameter,
    Function,
    Method,
    Constant,
    Static,
}

#[derive(Debug, Clone)]
pub struct AnalysisSemanticEntry {
    pub span: kernc_utils::Span,
    pub definition_span: kernc_utils::Span,
    pub kind: AnalysisSemanticKind,
    pub role: AnalysisSemanticRole,
    pub is_mut: bool,
    pub is_pub: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisCompletionKind {
    Variable,
    Function,
    Module,
    Struct,
    Union,
    Enum,
    Trait,
    TypeAlias,
    Constant,
    Static,
    TypeParameter,
}

#[derive(Debug, Clone)]
pub struct AnalysisCompletionItem {
    pub label: String,
    pub kind: AnalysisCompletionKind,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AnalysisParameterInformation {
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct AnalysisSignatureInformation {
    pub label: String,
    pub parameters: Vec<AnalysisParameterInformation>,
}

#[derive(Debug, Clone)]
pub struct AnalysisSignatureHelp {
    pub signatures: Vec<AnalysisSignatureInformation>,
    pub active_signature: usize,
    pub active_parameter: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisSymbolKind {
    Module,
    Namespace,
    Function,
    Method,
    Struct,
    Union,
    Enum,
    Trait,
    TypeAlias,
    Constant,
    Static,
}

#[derive(Debug, Clone)]
pub struct AnalysisSymbol {
    pub name: String,
    pub kind: AnalysisSymbolKind,
    pub span: kernc_utils::Span,
    pub selection_span: kernc_utils::Span,
    pub detail: Option<String>,
    pub children: Vec<AnalysisSymbol>,
}

pub struct AnalysisArtifact {
    pub session: Session,
    pub succeeded: bool,
    pub symbols: Vec<AnalysisSymbol>,
    pub references: Vec<AnalysisReference>,
    pub hovers: Vec<AnalysisHover>,
    pub semantic_entries: Vec<AnalysisSemanticEntry>,
    asts: Vec<(DefId, ast::Module)>,
    resolved_globals: Vec<ResolvedGlobalType>,
    completion_model: completion::CompletionModel,
    signature_model: signature::SignatureModel,
}

pub struct AnalysisOutline {
    pub session: Session,
    pub symbols: Vec<AnalysisSymbol>,
}

pub struct ParsedModuleArtifact {
    session: Session,
    modules: Vec<ParsedModule>,
}

#[derive(Clone)]
struct ParsedModule {
    name: String,
    file_id: kernc_utils::FileId,
    ast: ast::Module,
}

pub struct StructureArtifact {
    session: Session,
    asts: Vec<(DefId, ast::Module)>,
    snapshot: SemaStructureSnapshot,
    completion_model: completion::CompletionModel,
}

pub struct CompilerDriver {
    pub options: CompileOptions,
}

#[derive(Debug, Clone)]
struct ResolvedGlobalType {
    scope_id: ScopeId,
    name: SymbolId,
    ty: TypeId,
}

struct TempFileGuard {
    path: String,
}

struct LinkTarget {
    triple: String,
    is_windows: bool,
    is_darwin: bool,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl AnalysisArtifact {
    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model
            .completion_items(&self.session, target_path, offset)
    }

    pub fn signature_help(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Option<AnalysisSignatureHelp> {
        self.signature_model
            .signature_help(&self.session, target_path, offset)
    }
}

impl ParsedModuleArtifact {
    pub fn requires_body_completion(&self, target_path: &Path, offset: usize) -> bool {
        completion::parsed_requires_body_completion(
            &self.session,
            &self.modules,
            target_path,
            offset,
        )
    }
}

impl StructureArtifact {
    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model
            .completion_items(&self.session, target_path, offset)
    }
}

impl CompilerDriver {
    pub fn new(options: CompileOptions) -> Self {
        Self { options }
    }

    pub fn compile(&self) -> bool {
        if self.options.driver_mode == DriverMode::LinkOnly {
            return self.link_only();
        }

        let Some(input_file) = self.options.input_file.as_deref() else {
            eprintln!("Error: compile mode requires a source input.");
            return false;
        };

        let mut session = Session::new();
        let Some(mut ctx) = self.analyze(&mut session, input_file) else {
            return false;
        };

        let Some(mast_module) = self.lower_module(&mut ctx) else {
            return false;
        };

        if let Some(metadata_output) = self.options.metadata_output.as_deref()
            && let Err(err) = metadata::emit_package_metadata(
                &ctx,
                Path::new(metadata_output),
                self.options
                    .metadata_package_name
                    .as_deref()
                    .or(self.options.root_module_name.as_deref())
                    .unwrap_or("root"),
                self.options.metadata_package_version.as_deref(),
            )
        {
            eprintln!("Error: Failed to emit kmeta snapshot: {}", err);
            return false;
        }

        let codegen_ctx = Context::create();
        let mut codegen = CodeGenerator::new(
            &codegen_ctx,
            &self.module_name_for_codegen(input_file),
            &mut *ctx.sess,
            &ctx.type_registry,
        );

        codegen.set_asm_dialect(match self.options.asm_dialect {
            AsmDialect::Intel => InlineAsmDialect::Intel,
            AsmDialect::Att => InlineAsmDialect::ATT,
        });

        codegen.compile(&mast_module);

        if self.options.driver_mode == DriverMode::EmitLlvmIr {
            return match codegen.print_ir() {
                Ok(()) => true,
                Err(err) => {
                    eprintln!("Error: Failed to print LLVM IR: {}", err);
                    false
                }
            };
        }

        let target = self.normalized_target();
        let link_input_path = self.prepare_link_input_path(&target);
        let _guard = self.temp_link_input_guard(&link_input_path);

        if let Err(err) =
            codegen.emit_to_file(&target.triple, &link_input_path, self.options.opt_level)
        {
            eprintln!("Error: LLVM failed to generate intermediate file: {}", err);
            return false;
        }

        if self.options.driver_mode.emits_linker_input() {
            println!(
                "Successfully emitted linker input to `{}`",
                self.options.output_file
            );
            return true;
        }

        self.run_link_command(Some(&link_input_path), &target, "Successfully compiled")
    }

    pub fn analyze<'a>(
        &self,
        session: &'a mut Session,
        input_file: &str,
    ) -> Option<SemaContext<'a>> {
        self.analyze_with_overrides(session, input_file, &SourceOverrides::new())
    }

    pub fn analyze_with_overrides<'a>(
        &self,
        session: &'a mut Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<SemaContext<'a>> {
        session.apply_options(&self.options);

        let mut ctx = self.build_sema_context(session);
        let asts = self.load_asts(&mut ctx, input_file, source_overrides)?;
        let snapshot = self.build_structure_snapshot(&mut ctx, asts)?;
        drop(ctx);

        let mut ctx = self.build_sema_context(session);
        ctx.restore_structure(snapshot);
        if !self.run_body_pipeline(&mut ctx) {
            return None;
        }

        Some(ctx)
    }

    pub fn analyze_structure(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<StructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_analyze_structure(session, input_file, source_overrides)
            .ok()
    }

    fn lower_module<'a>(&self, ctx: &mut SemaContext<'a>) -> Option<kernc_mast::MastModule> {
        let mut lowerer = Lowerer::new(ctx);
        let module = lowerer.lower_all();
        if !Self::report_diagnostics_if_errors(lowerer.context()) {
            return None;
        }
        Some(module)
    }

    fn module_name_for_codegen(&self, input_file: &str) -> String {
        Path::new(input_file)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("kern_module")
            .to_string()
    }

    fn report_diagnostics_if_errors(ctx: &mut SemaContext<'_>) -> bool {
        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }
        true
    }
}
