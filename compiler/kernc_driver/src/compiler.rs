use inkwell::context::Context;
use std::env;
use std::process::Command;

use kernc_codegen::CodeGenerator;
use kernc_lower::Lowerer;
use kernc_sema::BuiltinInjector;
use kernc_sema::SemaContext;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::passes::{Collector, ImportResolver, TypeResolver, LinkageChecker};
use kernc_utils::Session;
use kernc_utils::config::{AsmDialect, CompileOptions, DriverMode, LinkProfile};

use crate::loader::ModuleLoader;

pub struct CompilerDriver {
    pub options: CompileOptions,
}

/// 临时文件守卫 (RAII)
/// 当变量离开作用域时，自动删除产生的临时文件
struct TempFileGuard {
    path: String,
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

        // 1. 初始化最底层的全局 Session (掌管文件、报错、配置)
        let mut session = Session::new();
        session.apply_options(&self.options);

        // 2. 初始化语义分析中枢
        let mut ctx = SemaContext::new(&mut session);

        ctx.module_aliases = self.options.module_aliases.clone();

        // 3. 注入内置类型 (void, i32, bool 等)
        let mut builtin = BuiltinInjector::new(&mut ctx);
        builtin.inject();

        // 4. 加载、解析和宏剪枝
        let asts = {
            let mut loader = ModuleLoader::new(&mut ctx);
            // ModuleLoader 内部已经接管了文件读取和报错
            let input_file = self
                .options
                .input_file
                .as_deref()
                .expect("compile mode requires a source input");
            if loader.load_root(input_file).is_none() {
                ctx.sess.print_diagnostics();
                return false;
            }
            std::mem::take(&mut loader.asts)
        };

        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }

        ctx.inject_alias_roots();

        // 5. 语义分析流水线
        let mut collector = Collector::new(&mut ctx);
        for (mod_id, ast) in asts {
            collector.collect_ast(mod_id, &ast);
        }
        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }

        let mut import_resolver = ImportResolver::new(&mut ctx);
        import_resolver.resolve_all();
        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }

        let mut type_resolver = TypeResolver::new(&mut ctx);
        type_resolver.resolve_all();
        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }

        let mut typeck = TypeckDriver::new(&mut ctx);
        typeck.check_all();
        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }

        let mut linkage_checker = LinkageChecker::new(&mut ctx);
        linkage_checker.check_all();
        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }

        // 6. 中端降级
        let mut lowerer = Lowerer::new(&mut ctx);
        let mast_module = lowerer.lower_all();

        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }

        // 7. 后端代码生成
        let codegen_ctx = Context::create();
        let mod_name = std::path::Path::new(
            self.options
                .input_file
                .as_deref()
                .expect("compile mode requires a source input"),
        )
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("kern_module");

        let mut codegen = CodeGenerator::new(
            &codegen_ctx,
            mod_name,
            &mut *ctx.sess,
            &ctx.type_registry,
            &ctx.defs,
        );

        codegen.asm_dialect = match self.options.asm_dialect {
            AsmDialect::Intel => inkwell::InlineAsmDialect::Intel,
            AsmDialect::Att => inkwell::InlineAsmDialect::ATT,
        };

        codegen.compile(&mast_module);

        if self.options.driver_mode == DriverMode::EmitLlvmIr {
            codegen.print_ir();
            return true;
        }

        // 处理跨平台逻辑
        let triple_str = self.options.target.triple.to_string();
        let is_windows = triple_str.contains("windows");
        let is_darwin = triple_str.contains("darwin") || triple_str.contains("macosx");
        let triple_str = if is_darwin {
            normalize_darwin_triple_str(&triple_str)
        } else {
            triple_str
        };

        let link_input_path = if self.options.driver_mode.emits_linker_input() {
            self.options.output_file.clone()
        } else {
            self.make_temp_link_input_path(is_windows)
        };

        let _guard = if self.options.driver_mode.emits_linker_input() {
            None
        } else {
            Some(TempFileGuard {
                path: link_input_path.clone(),
            })
        };

        if let Err(e) = codegen.emit_to_file(&triple_str, &link_input_path, self.options.opt_level) {
            eprintln!("Error: LLVM failed to generate intermediate file: {}", e);
            return false;
        }

        if self.options.driver_mode.emits_linker_input() {
            println!("Successfully emitted linker input to `{}`", self.options.output_file);
            return true;
        }

        println!("Linking for target: {} ...", triple_str);

        let mut cmd =
            self.build_link_command(Some(&link_input_path), &triple_str, is_windows, is_darwin);
        self.maybe_print_link_command(&cmd);

        match cmd
            .status()
        {
            Ok(s) if s.success() => {
                println!("Successfully compiled to `{}`", self.options.output_file);
                true
            }
            Ok(s) => {
                eprintln!("Error: Linker failed with exit code {}", s);
                false
            }
            Err(e) => {
                let cc_compiler = self.resolve_linker_driver(is_windows);
                eprintln!(
                    "Error: Failed to invoke linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cc_compiler, e
                );
                false
            }
        }
    }

    fn link_only(&self) -> bool {
        if self.options.linker_inputs.is_empty() {
            eprintln!("Error: `--link-only` requires at least one `--link-input`.");
            return false;
        }

        let triple_str = self.options.target.triple.to_string();
        let is_windows = triple_str.contains("windows");
        let is_darwin = triple_str.contains("darwin") || triple_str.contains("macosx");
        let triple_str = if is_darwin {
            normalize_darwin_triple_str(&triple_str)
        } else {
            triple_str
        };

        println!("Linking for target: {} ...", triple_str);

        let mut cmd = self.build_link_command(None, &triple_str, is_windows, is_darwin);
        self.maybe_print_link_command(&cmd);

        match cmd
            .status()
        {
            Ok(s) if s.success() => {
                println!("Successfully linked to `{}`", self.options.output_file);
                true
            }
            Ok(s) => {
                eprintln!("Error: Linker failed with exit code {}", s);
                false
            }
            Err(e) => {
                let cc_compiler = self.resolve_linker_driver(is_windows);
                eprintln!(
                    "Error: Failed to invoke linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cc_compiler, e
                );
                false
            }
        }
    }

    fn make_temp_link_input_path(&self, is_windows: bool) -> String {
        let tmp_ext = if is_windows { "ll" } else { "o" };
        format!("{}.tmp.{}", self.options.output_file, tmp_ext)
    }

    fn resolve_linker_driver(&self, is_windows: bool) -> String {
        if is_windows && self.options.linker_cmd == "cc" {
            "clang".to_string()
        } else {
            self.options.linker_cmd.clone()
        }
    }

    fn build_link_command(
        &self,
        link_input_path: Option<&str>,
        _target_triple: &str,
        is_windows: bool,
        is_darwin: bool,
    ) -> Command {
        let cc_compiler = self.resolve_linker_driver(is_windows);
        let mut cmd = Command::new(&cc_compiler);

        if let Some(link_input_path) = link_input_path {
            cmd.arg(link_input_path);
        }

        for input in &self.options.linker_inputs {
            cmd.arg(input);
        }

        cmd.arg("-o").arg(&self.options.output_file);

        self.apply_link_profile(&mut cmd, is_windows, is_darwin);

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
                    cmd.arg("-lkernel32");
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
    if triple_str.contains("macosx") && triple_str.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        return triple_str.to_string();
    }

    if triple_str.contains("darwin") && triple_str.chars().last().is_some_and(|c| c.is_ascii_digit()) {
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
