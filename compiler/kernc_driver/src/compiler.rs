use inkwell::context::Context;
use std::process::Command;

use kernc_codegen::CodeGenerator;
use kernc_lower::Lowerer;
use kernc_sema::BuiltinInjector;
use kernc_sema::SemaContext;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::passes::{Collector, ImportResolver, TypeResolver};
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

        // 6. 中端降级
        let mut lowerer = Lowerer::new(&mut ctx);
        let mast_module = lowerer.lower_all();

        // 7. 后端代码生成
        let codegen_ctx = Context::create();
        let mod_name = std::path::Path::new(&self.options.input_file)
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("kern_module");

        let resolve_fn = |sym| ctx.sess.interner.resolve(sym).unwrap_or("<unknown>");

        let mut codegen = CodeGenerator::new(
            &codegen_ctx,
            mod_name,
            &ctx.type_registry,
            &ctx.defs,
            &resolve_fn,
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

        // 8. 输出目标文件
        let obj_path_str = format!("{}.tmp.o", self.options.output_file);
        let _guard = TempFileGuard {
            path: obj_path_str.clone(),
        };

        if let Err(e) = codegen.emit_to_file(
            &self.options.target.triple.to_string(),
            &obj_path_str,
            self.options.opt_level,
        ) {
            eprintln!("Error: LLVM failed to generate object file: {}", e);
            return false;
        }

        // 9. 链接器调用
        println!("Linking...");
        let cc_compiler = self.options.linker_cmd.clone();
        let mut cmd = Command::new(&cc_compiler);

        cmd.arg(&obj_path_str)
            .arg("-no-pie")
            .arg("-o")
            .arg(&self.options.output_file);

        if !self.options.link_libc {
            cmd.arg("-nostdlib");
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
                    "Error: Failed to invoke linker (`{}`). Make sure a C compiler is installed. ({})",
                    cc_compiler, e
                );
                false
            }
        }
    }
}
