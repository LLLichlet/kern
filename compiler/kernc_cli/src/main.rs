use kernc_driver::CompilerDriver;
use kernc_utils::config::{
    AsmDialect, CompileOptions, DriverMode, LibraryBundle, LlvmIrStage, LtoMode, OptLevel,
    RuntimeEntry, TargetMachine, apply_configured_library_aliases, inject_driver_condition_defines,
    validate_compile_options,
};
use std::env;
use std::path::Path;
use std::process;

fn set_driver_mode(options: &mut CompileOptions, requested: DriverMode, flag: &str) {
    if options.driver_mode != DriverMode::CompileAndLink && options.driver_mode != requested {
        eprintln!(
            "Error: `{}` conflicts with a previously selected driver mode.",
            flag
        );
        process::exit(1);
    }
    options.driver_mode = requested;
}

fn print_usage(program_name: &str) {
    let version = env!("CARGO_PKG_VERSION");

    println!("Kern Compiler v{}", version);
    println!("Usage: {} [OPTIONS] [input.rn]\n", program_name);

    println!("Build Options:");
    println!("  -o <file>            Write output to <file>");
    println!("  -c                   Emit linker input and skip the final system link step");
    println!("  --link-only          Skip frontend/codegen and invoke the linker driver only");
    println!("  --define <key=val>   Define a variable for conditional compilation");
    println!("  --module-path <name=path>");
    println!(
        "                       Map a module name to a physical directory (e.g., --module-path std=./library/std)"
    );
    println!("  --module-interface-path <name=path>");
    println!("                       Map a module name to an imported metadata root");
    println!("  --metadata-output <dir>");
    println!("                       Emit a module metadata snapshot directory");
    println!("  --module-root-name <name>");
    println!("                       Override the compiled root module name");
    println!("  -O<0-3>              Set optimization level (default: O0)");

    println!("\nTargeting & Codegen:");
    println!("  --target <T>         Set target triple (e.g. x86_64-unknown-linux-gnu)");
    println!("  --asm-dialect <D>    Set assembly dialect: intel (default) or att");
    println!("  --codegen-units <N>  Split code generation into N lowered codegen units");
    println!("  --lto <M>            Cross-CGU optimization mode: none, full, thin");
    println!("  --link-driver <cmd>  Set the linker driver command (default: $CC or cc)");
    println!("  --runtime-entry <m>  Runtime entry contract: none, rt, crt");
    println!("  --runtime-libc <b>   Whether libc is linked: yes, no");
    println!("  --library-bundle <b> Library bundle: none, base, std");
    println!("  --link-input <path>  Add an extra linker input (.o/.a/.so/response file)");
    println!("  --link-search <dir>  Add a linker search path");
    println!("  --link-lib <name>    Link against a library");
    println!("  -L <dir>             Add a linker search path");
    println!("  -l <name>            Link against a library");
    println!("  --link-arg <arg>     Pass a raw argument through to the linker driver");
    println!("  --entry-symbol <s>   Override the default entry symbol used by kernc");
    println!("  --print-link-command Print the resolved linker command before execution");
    println!("  --emit-llvm[=S]      Print LLVM IR stage S to stdout (raw by default)");
    println!("                       S: raw, verified, optimized");
    println!("  --timings            Print compiler phase timings and cache stats");

    println!("\nInformation:");
    println!("  -v, --version        Display version information and exit");
    println!("  -h, --help           Display this help and exit");
}

fn cli_error(message: impl Into<String>) -> ! {
    eprintln!("Error: {}", message.into());
    process::exit(1);
}

fn next_option_value(args: &mut env::Args, flag: &str, value_name: &str) -> String {
    args.next()
        .unwrap_or_else(|| cli_error(format!("Expected {} after `{}`.", value_name, flag)))
}

fn parse_target_machine(value: &str) -> TargetMachine {
    TargetMachine::new(value).unwrap_or_else(|e| cli_error(format!("Invalid target triple: {}", e)))
}

fn parse_asm_dialect(value: &str) -> AsmDialect {
    match value {
        "intel" => AsmDialect::Intel,
        "att" => AsmDialect::Att,
        _ => cli_error(format!(
            "Invalid asm dialect `{}`. Expected one of: intel, att.",
            value
        )),
    }
}

fn parse_runtime_entry(value: &str) -> RuntimeEntry {
    RuntimeEntry::parse(value).unwrap_or_else(|err| cli_error(err))
}

fn parse_library_bundle(value: &str) -> LibraryBundle {
    LibraryBundle::parse(value).unwrap_or_else(|err| cli_error(err))
}

fn parse_llvm_ir_stage(value: &str) -> LlvmIrStage {
    LlvmIrStage::parse(value).unwrap_or_else(|err| cli_error(err))
}

fn parse_lto_mode(value: &str) -> LtoMode {
    LtoMode::parse(value).unwrap_or_else(|err| cli_error(err))
}

fn parse_yes_no(value: &str, flag: &str) -> bool {
    match value {
        "yes" | "true" | "on" => true,
        "no" | "false" | "off" => false,
        _ => cli_error(format!(
            "Invalid value `{value}` for `{flag}`. Expected one of: yes, no."
        )),
    }
}

fn parse_nonzero_usize(value: &str, flag: &str) -> usize {
    let parsed = value
        .parse::<usize>()
        .unwrap_or_else(|_| cli_error(format!("Invalid value `{value}` for `{flag}`.")));
    if parsed == 0 {
        cli_error(format!(
            "Invalid value `{value}` for `{flag}`. Expected a positive integer."
        ));
    }
    parsed
}

fn parse_key_value(raw: String, flag: &str, expected: &str) -> (String, String) {
    match raw.split_once('=') {
        Some((key, value)) if !key.is_empty() && !value.is_empty() => {
            (key.to_string(), value.to_string())
        }
        _ => cli_error(format!(
            "Invalid argument for `{}`. Expected `{}`.",
            flag, expected
        )),
    }
}

fn consume_short_or_attached_value(
    arg: &str,
    prefix: &str,
    args: &mut env::Args,
    value_name: &str,
) -> Option<String> {
    let value = arg.strip_prefix(prefix)?;
    if value.is_empty() {
        Some(next_option_value(args, prefix, value_name))
    } else {
        Some(value.to_string())
    }
}

fn consume_long_option_value(
    arg: &str,
    flag: &str,
    args: &mut env::Args,
    value_name: &str,
) -> Option<String> {
    if arg == flag {
        return Some(next_option_value(args, flag, value_name));
    }

    let prefix = format!("{flag}=");
    arg.strip_prefix(&prefix).map(|value| value.to_string())
}

fn set_default_output_file(options: &mut CompileOptions) {
    if !options.output_file.is_empty() {
        return;
    }

    match options.driver_mode {
        DriverMode::CompileOnly => {
            let stem = options
                .input_file
                .as_deref()
                .and_then(|input| Path::new(input).file_stem())
                .and_then(|s| s.to_str())
                .unwrap_or("a.out");
            options.output_file = format!("{}.o", stem);
        }
        _ => {
            options.output_file = "a.out".to_string();
        }
    }
}

fn validate_mode_inputs(
    program_name: &str,
    options: &CompileOptions,
    positional_source: &Option<String>,
) {
    if options.driver_mode.needs_source_input() && positional_source.is_none() {
        eprintln!("Error: No input file specified.");
        print_usage(program_name);
        process::exit(1);
    }

    if options.driver_mode == DriverMode::LinkOnly && positional_source.is_some() {
        eprintln!("Error: `--link-only` does not accept a source input.");
        eprintln!("Hint: Pass object files, archives, or shared libraries via `--link-input`.");
        process::exit(1);
    }

    validate_compile_options(options).unwrap_or_else(|err| cli_error(err));
}

fn parse_args() -> CompileOptions {
    let mut args = env::args();
    let program_name = args.next().unwrap_or_else(|| "kernc".to_string());

    let mut options = CompileOptions::default();

    // Read environment variables before parsing CLI arguments.
    if let Ok(cc_env) = env::var("CC") {
        options.linker_cmd = cc_env;
    }

    let mut positional_source: Option<String> = None;

    while let Some(arg) = args.next() {
        if let Some(value) =
            consume_long_option_value(&arg, "--metadata-output", &mut args, "directory")
        {
            options.metadata_output = Some(value);
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--module-root-name", &mut args, "module name")
        {
            options.root_module_name = Some(value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--target", &mut args, "target triple")
        {
            options.target = parse_target_machine(&value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--asm-dialect", &mut args, "dialect")
        {
            options.asm_dialect = parse_asm_dialect(&value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--codegen-units", &mut args, "count")
        {
            options.codegen_units = parse_nonzero_usize(&value, "--codegen-units");
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--lto", &mut args, "mode") {
            options.lto_mode = parse_lto_mode(&value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--link-driver", &mut args, "command")
        {
            options.linker_cmd = value;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--emit-llvm=") {
            set_driver_mode(&mut options, DriverMode::EmitLlvmIr, "--emit-llvm");
            options.emit_llvm_stage = parse_llvm_ir_stage(value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--runtime-entry", &mut args, "mode") {
            options.runtime_entry = parse_runtime_entry(&value);
            continue;
        }
        if consume_long_option_value(&arg, "--runtime-provider", &mut args, "provider").is_some() {
            cli_error(
                "`--runtime-provider` has been removed; select `sys`/`rt` implementations via module paths or packages, and use `--runtime-libc` only for libc linkage",
            );
        }
        if let Some(value) = consume_long_option_value(&arg, "--runtime-libc", &mut args, "yes|no")
        {
            options.runtime_libc = parse_yes_no(&value, "--runtime-libc");
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--library-bundle", &mut args, "bundle")
        {
            options.library_bundle = parse_library_bundle(&value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--link-input", &mut args, "path") {
            options.linker_inputs.push(value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--link-search", &mut args, "path") {
            options.linker_search_paths.push(value);
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--link-lib", &mut args, "library name")
        {
            options.linker_libraries.push(value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--link-arg", &mut args, "argument") {
            options.linker_args.push(value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--entry-symbol", &mut args, "symbol")
        {
            options.entry_symbol = Some(value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--define", &mut args, "`key=value`") {
            let (key, value) = parse_key_value(value, "--define", "key=value");
            options.custom_defines.insert(key, value);
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--module-path", &mut args, "`name=path`")
        {
            let (name, path) = parse_key_value(value, "--module-path", "name=path");
            options.module_aliases.insert(name, path);
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--module-interface-path", &mut args, "`name=path`")
        {
            let (name, path) = parse_key_value(value, "--module-interface-path", "name=path");
            options.module_interface_aliases.insert(name, path);
            continue;
        }

        match arg.as_str() {
            "-h" | "--help" => {
                print_usage(&program_name);
                process::exit(0);
            }
            "-v" | "--version" => {
                println!("kernc version {}", env!("CARGO_PKG_VERSION"));
                process::exit(0);
            }
            "-o" => options.output_file = next_option_value(&mut args, "-o", "file name"),
            "-c" => set_driver_mode(&mut options, DriverMode::CompileOnly, "-c"),
            "--link-only" => set_driver_mode(&mut options, DriverMode::LinkOnly, "--link-only"),
            "-O0" => options.opt_level = OptLevel::O0,
            "-O1" => options.opt_level = OptLevel::O1,
            "-O2" => options.opt_level = OptLevel::O2,
            "-O3" => options.opt_level = OptLevel::O3,
            "--emit-llvm" => set_driver_mode(&mut options, DriverMode::EmitLlvmIr, "--emit-llvm"),
            "--timings" => options.report_timings = true,
            "--print-link-command" => options.print_link_command = true,
            _ => {
                if let Some(path) = consume_short_or_attached_value(&arg, "-L", &mut args, "path") {
                    options.linker_search_paths.push(path);
                    continue;
                }

                if let Some(lib) =
                    consume_short_or_attached_value(&arg, "-l", &mut args, "library name")
                {
                    options.linker_libraries.push(lib);
                    continue;
                }

                if arg.starts_with('-') {
                    cli_error(format!("Unrecognized option `{}`", arg));
                }
                if positional_source.is_some() {
                    cli_error("Multiple input files are not supported yet.");
                }
                positional_source = Some(arg);
            }
        }
    }

    validate_mode_inputs(&program_name, &options, &positional_source);
    options.input_file = positional_source;
    set_default_output_file(&mut options);
    inject_driver_condition_defines(&mut options);
    apply_configured_library_aliases(&mut options);

    options
}

fn main() {
    let options = parse_args();
    let driver = CompilerDriver::new(options);

    if !driver.compile() {
        process::exit(1);
    }
}
