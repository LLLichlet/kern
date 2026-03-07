use kernc::context::Context;
use kernc::parser::Parser;
use kernc::sema::collect::Collector;
use kernc::sema::resolve_types::TypeResolver;
use kernc::sema::typeck::TypeckDriver;
use kernc::mast::lower::Lowerer;
use kernc::codegen::llvm::CodeGenerator;
use inkwell::context::Context as LlvmContext;

fn main() {
    // 1. 准备一段硬核且语法绝对正确的 Kern 源码测试
    let source_code = r#"
        static OFFSET: i32 = 100;

        type Point = struct {
            x: i32,
            y: i32,
        };

        fn add_offset(val: i32) i32 {
            return val + OFFSET;
        }

        fn main() i32 {
            let p: mut Point = .{ x: 10, y: 20 };
            p.x += 5;
            
            let result: mut i32 = 0;
            if (p.x > 10) {
                result = add_offset(p.x);
            } else {
                result = p.y;
            }
            
            return result;
        }
    "#;

    // 2. 初始化编译上下文
    let mut ctx = Context::new();
    
    // 🌟 修复点 1: 向 SourceManager 注册文件并获取 file_id
    // 这对于多文件编译和错误定位至关重要！
    let file_id = ctx.source_manager.add_file("test.kn".to_string(), source_code.to_string());
    
    // 注入内置函数和 Traits 
    let mut builtin = kernc::sema::builtin::BuiltinInjector::new(&mut ctx);
    builtin.inject();

    // 3. 词法与语法分析
    // 🌟 修复点 2: 传入获取到的 file_id
    let mut parser = Parser::new(source_code, file_id, &mut ctx);
    
    let module_ast = match parser.parse_module() {
        Ok(ast) => ast,
        Err(_) => {
            // 如果发生致命语法错误，打印并退出
            ctx.print_diagnostics();
            return;
        }
    };

    // 如果 Parser 有非致命错误(比如忘了分号)，提前拦截
    if ctx.has_errors() {
        ctx.print_diagnostics();
        return;
    }

    // 4. 语义分析 Pass 1: 符号收集
    let mut collector = Collector::new(&mut ctx);
    let mod_id = collector.collect_module(&module_ast);

    // 5. 语义分析 Pass 2: 类型解析
    let mut resolver = TypeResolver::new(&mut ctx);
    resolver.resolve_all();

    if ctx.has_errors() {
        ctx.print_diagnostics();
        return;
    }

    // 6. 语义分析 Pass 3: 类型检查与推导
    let mut typeck = TypeckDriver::new(&mut ctx);
    typeck.check_all();

    // 🌟 修复点 3: 使用正确的 API 打印错误
    if ctx.has_errors() {
        ctx.print_diagnostics();
        return;
    }

    // 7. MAST 降级与单态化
    let mut lowerer = Lowerer::new(&mut ctx);
    let mast_module = lowerer.lower_all();
    
    // 调试专用：你可以解开这个注释，看看你精心设计的 MAST 长什么样！
    // println!("========================================");
    // println!("🌳 MAST Generated:");
    // println!("{:#?}", mast_module);
    // println!("========================================\n");

    // 8. LLVM 代码生成
    let llvm_ctx = LlvmContext::create();
    let mut codegen = CodeGenerator::new(&llvm_ctx, "kern_module", &ctx.type_registry);
    
    codegen.compile(&mast_module);

    println!("========================================");
    println!("🎉 Compile Success! Generated LLVM IR:");
    println!("========================================");
    
    codegen.print_ir();
}