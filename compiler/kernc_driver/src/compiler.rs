use inkwell::context::Context;
use std::process::Command;

use kernc_codegen::CodeGenerator;
use kernc_lower::Lowerer;
use kernc_sema::BuiltinInjector;
use kernc_sema::SemaContext;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::passes::{Collector, ImportResolver, TypeResolver, LinkageChecker};
use kernc_utils::Session;
use kernc_utils::config::{AsmDialect, CompileOptions};

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
        // 1. 初始化最底层的全局 Session (掌管文件、报错、配置)
        let mut session = Session::new();

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
            if loader.load_root(&self.options.input_file).is_none() {
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
        let mod_name = std::path::Path::new(&self.options.input_file)
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

        if self.options.emit_llvm_ir {
            codegen.print_ir();
            return true;
        }

        // 处理跨平台逻辑
        let triple_str = self.options.target.triple.to_string();
        let is_windows = triple_str.contains("windows");

        // 8. 根据平台决定临时文件格式：Windows 走 IR，类 Unix 走 Object
        let tmp_ext = if is_windows { "ll" } else { "o" };
        let obj_path_str = format!("{}.tmp.{}", self.options.output_file, tmp_ext);
        
        let _guard = TempFileGuard {
            path: obj_path_str.clone(),
        };

        if let Err(e) = codegen.emit_to_file(&triple_str, &obj_path_str, self.options.opt_level) {
            eprintln!("Error: LLVM failed to generate intermediate file: {}", e);
            return false;
        }

        // 9. 智能链接器调用
        println!("Linking for target: {} ...", triple_str);
        
        // 如果在 Windows 下且没有显式指定 --cc，我们强制使用 clang，因为它是能解析 .ll 的最佳驱动器
        let cc_compiler = if is_windows && self.options.linker_cmd == "cc" {
            "clang".to_string()
        } else {
            self.options.linker_cmd.clone()
        };

        let mut cmd = Command::new(&cc_compiler);

        cmd.arg(&obj_path_str)
            .arg("-o")
            .arg(&self.options.output_file);

        // 处理平台专属的链接选项
        if is_windows {
            // 静音由于三元组小版本不一致导致的警告
            cmd.arg("-Wno-override-module");
            // Windows 下不需要且不支持 -no-pie
            if !self.options.link_libc {
                cmd.arg("-nostdlib");
                cmd.arg("-lkernel32"); // 链接必需的系统底层 API
            }
        } else {
            // Linux / macOS 下
            cmd.arg("-no-pie"); // 针对部分 Linux GCC 环境的配置
            if !self.options.link_libc {
                cmd.arg("-nostdlib");
            }
        }

        match cmd.status() {
            Ok(s) if s.success() => {
                println!("Successfully compiled to `{}`", self.options.output_file);
                true
            }
            Ok(s) => {
                eprintln!("Error: Linker failed with exit code {}", s);
                false
            }
            Err(e) => {
                eprintln!(
                    "Error: Failed to invoke linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cc_compiler, e
                );
                false
            }
        }
    }
}
