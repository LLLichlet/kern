use std::env;
use std::process::Command;

use kernc_ast as ast;
use kernc_codegen::{CodeGenerator, Context, InlineAsmDialect};
use kernc_lower::Lowerer;
use kernc_sema::BuiltinInjector;
use kernc_sema::SemaContext;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::def::DefId;
use kernc_sema::passes::{Collector, ImportResolver, LinkageChecker, TypeResolver};
use kernc_utils::Session;
use kernc_utils::config::{AsmDialect, CompileOptions, DriverMode, LinkProfile};

use crate::loader::ModuleLoader;
use crate::metadata;

pub struct CompilerDriver {
    pub options: CompileOptions,
}

/// 临时文件守卫 (RAII)
/// 当变量离开作用域时，自动删除产生的临时文件
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
                std::path::Path::new(metadata_output),
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

        if let Err(e) =
            codegen.emit_to_file(&target.triple, &link_input_path, self.options.opt_level)
        {
            eprintln!("Error: LLVM failed to generate intermediate file: {}", e);
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
        session.apply_options(&self.options);

        let mut ctx = self.build_sema_context(session);
        let asts = self.load_asts(&mut ctx, input_file)?;
        if !self.run_sema_pipeline(&mut ctx, asts) {
            return None;
        }

        Some(ctx)
    }

    fn link_only(&self) -> bool {
        if self.options.linker_inputs.is_empty() {
            eprintln!("Error: `--link-only` requires at least one `--link-input`.");
            return false;
        }

        let target = self.normalized_target();
        self.run_link_command(None, &target, "Successfully linked")
    }

    fn build_sema_context<'a>(&self, session: &'a mut Session) -> SemaContext<'a> {
        let mut ctx = SemaContext::new(session);
        ctx.module_aliases = self.options.module_aliases.clone();
        ctx.module_interface_aliases = self.options.module_interface_aliases.clone();

        let mut builtin = BuiltinInjector::new(&mut ctx);
        builtin.inject();
        ctx
    }

    fn load_asts<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        input_file: &str,
    ) -> Option<Vec<(DefId, ast::Module)>> {
        let mut loader = ModuleLoader::new(ctx);
        let root_name = loader
            .ctx
            .intern(self.options.root_module_name.as_deref().unwrap_or("root"));
        if loader.load_root(input_file, root_name).is_none() {
            loader.ctx.sess.print_diagnostics();
            return None;
        }
        if !Self::report_diagnostics_if_errors(loader.ctx) {
            return None;
        }

        loader.ctx.inject_alias_roots();
        Some(std::mem::take(&mut loader.asts))
    }

    fn run_sema_pipeline<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: Vec<(DefId, ast::Module)>,
    ) -> bool {
        let mut collector = Collector::new(ctx);
        for (mod_id, ast) in asts {
            collector.collect_ast(mod_id, &ast);
        }
        if !Self::report_diagnostics_if_errors(collector.context()) {
            return false;
        }

        let mut import_resolver = ImportResolver::new(collector.into_context());
        import_resolver.resolve_all();
        if !Self::report_diagnostics_if_errors(import_resolver.context()) {
            return false;
        }

        let mut type_resolver = TypeResolver::new(import_resolver.into_context());
        type_resolver.resolve_all();
        if !Self::report_diagnostics_if_errors(type_resolver.context()) {
            return false;
        }

        let mut typeck = TypeckDriver::new(type_resolver.into_context());
        typeck.check_all();
        let ctx = typeck.into_context();
        if !Self::report_diagnostics_if_errors(ctx) {
            return false;
        }

        let mut linkage_checker = LinkageChecker::new(ctx);
        linkage_checker.check_all();
        Self::report_diagnostics_if_errors(linkage_checker.context())
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
        std::path::Path::new(input_file)
            .file_stem()
            .and_then(|s| s.to_str())
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

    fn normalized_target(&self) -> LinkTarget {
        let raw_triple = self.options.target.triple.to_string();
        let is_windows = raw_triple.contains("windows");
        let is_darwin = raw_triple.contains("darwin") || raw_triple.contains("macosx");
        let triple = if is_darwin {
            normalize_darwin_triple_str(&raw_triple)
        } else {
            raw_triple
        };

        LinkTarget {
            triple,
            is_windows,
            is_darwin,
        }
    }

    fn prepare_link_input_path(&self, _target: &LinkTarget) -> String {
        if self.options.driver_mode.emits_linker_input() {
            self.options.output_file.clone()
        } else {
            self.make_temp_link_input_path()
        }
    }

    fn temp_link_input_guard(&self, link_input_path: &str) -> Option<TempFileGuard> {
        if self.options.driver_mode.emits_linker_input() {
            None
        } else {
            Some(TempFileGuard {
                path: link_input_path.to_string(),
            })
        }
    }

    fn run_link_command(
        &self,
        link_input_path: Option<&str>,
        target: &LinkTarget,
        success_prefix: &str,
    ) -> bool {
        println!("Linking for target: {} ...", target.triple);
        let mut cmd = self.build_link_command(link_input_path, target);
        self.maybe_print_link_command(&cmd);

        match cmd.status() {
            Ok(s) if s.success() => {
                println!("{} to `{}`", success_prefix, self.options.output_file);
                true
            }
            Ok(s) => {
                eprintln!("Error: Linker failed with exit code {}", s);
                false
            }
            Err(e) => {
                let cc_compiler = self.resolve_linker_driver(target.is_windows);
                eprintln!(
                    "Error: Failed to invoke linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cc_compiler, e
                );
                false
            }
        }
    }

    fn make_temp_link_input_path(&self) -> String {
        let tmp_ext = "o";
        format!("{}.tmp.{}", self.options.output_file, tmp_ext)
    }

    fn resolve_linker_driver(&self, is_windows: bool) -> String {
        if is_windows && self.options.linker_cmd == "cc" {
            "clang".to_string()
        } else {
            self.options.linker_cmd.clone()
        }
    }

    fn build_link_command(&self, link_input_path: Option<&str>, target: &LinkTarget) -> Command {
        let cc_compiler = self.resolve_linker_driver(target.is_windows);
        let mut cmd = Command::new(&cc_compiler);

        if let Some(link_input_path) = link_input_path {
            cmd.arg(link_input_path);
        }

        for input in &self.options.linker_inputs {
            cmd.arg(input);
        }

        cmd.arg("-o").arg(&self.options.output_file);

        self.apply_link_profile(&mut cmd, target.is_windows, target.is_darwin);

        for path in &self.options.linker_search_paths {
            cmd.arg(format!("-L{}", path));
        }

        for lib in &self.options.linker_libraries {
            cmd.arg(format!("-l{}", lib));
        }

        for arg in &self.options.linker_args {
            cmd.arg(arg);
        }

        cmd
    }

    fn apply_link_profile(&self, cmd: &mut Command, is_windows: bool, is_darwin: bool) {
        match self.options.link_profile {
            LinkProfile::None => {}
            LinkProfile::Hosted => {
                if !is_windows && !is_darwin {
                    cmd.arg("-no-pie");
                }
                if let Some(entry_symbol) = &self.options.entry_symbol {
                    cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                }
            }
            LinkProfile::Freestanding => {
                if is_windows {
                    cmd.arg("-Wno-override-module");
                    cmd.arg("-nostdlib");
                    cmd.arg("-Wl,/subsystem:console");
                    if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,/entry:{}", entry_symbol));
                    }
                } else if is_darwin {
                    cmd.arg("-nostdlib");
                    cmd.arg(format!(
                        "-Wl,-e,{}",
                        self.options.entry_symbol.as_deref().unwrap_or("_start")
                    ));
                } else {
                    cmd.arg("-no-pie");
                    cmd.arg("-nostdlib");
                    if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                    }
                }
            }
            LinkProfile::Kern => {
                if is_windows {
                    cmd.arg("-Wno-override-module");
                    cmd.arg("-nostdlib");
                    cmd.arg("-Wl,/subsystem:console");
                    cmd.arg("-lkernel32");
                    if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,/entry:{}", entry_symbol));
                    }
                } else if is_darwin {
                    cmd.arg("-nostdlib");
                    cmd.arg("-lSystem");
                    cmd.arg(format!(
                        "-Wl,-e,{}",
                        self.options.entry_symbol.as_deref().unwrap_or("_start")
                    ));
                } else {
                    cmd.arg("-no-pie");
                    cmd.arg("-nostdlib");
                    if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                    }
                }
            }
        }
    }

    fn maybe_print_link_command(&self, cmd: &Command) {
        if self.options.print_link_command {
            println!("Link command: {}", self.format_command(cmd));
        }
    }

    fn format_command(&self, cmd: &Command) -> String {
        let mut parts = Vec::new();
        parts.push(shell_quote(cmd.get_program().to_string_lossy().as_ref()));

        for arg in cmd.get_args() {
            parts.push(shell_quote(arg.to_string_lossy().as_ref()));
        }

        parts.join(" ")
    }
}

fn shell_quote(input: &str) -> String {
    if input.is_empty() {
        return "''".to_string();
    }

    if input
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '=' | ':'))
    {
        return input.to_string();
    }

    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

fn normalize_darwin_triple_str(triple_str: &str) -> String {
    if triple_str.contains("macosx")
        && triple_str
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_digit())
    {
        return triple_str.to_string();
    }

    if triple_str.contains("darwin")
        && triple_str
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_digit())
    {
        return triple_str.to_string();
    }

    let Some(version) = detect_darwin_deployment_target() else {
        return triple_str.to_string();
    };

    if let Some(prefix) = triple_str.strip_suffix("-darwin") {
        return format!("{}-macosx{}.0.0", prefix, version);
    }

    if let Some(prefix) = triple_str.strip_suffix("-macosx") {
        return format!("{}-macosx{}.0.0", prefix, version);
    }

    triple_str.to_string()
}

fn detect_darwin_deployment_target() -> Option<u16> {
    if let Ok(version) = env::var("MACOSX_DEPLOYMENT_TARGET") {
        return parse_darwin_deployment_target_major(&version);
    }

    let output = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8_lossy(&output.stdout);
    parse_darwin_deployment_target_major(version.trim())
}

fn parse_darwin_deployment_target_major(version: &str) -> Option<u16> {
    version.trim().split('.').next()?.parse().ok()
}
