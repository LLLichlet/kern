use kernc_driver::CompilerDriver;
use kernc_utils::config::{
    AsmDialect, CompileOptions, DriverMode, LibraryBundle, OptLevel, RuntimeEntry, RuntimeProvider,
    TargetMachine, inject_default_library_aliases, inject_driver_condition_defines,
    validate_runtime_options,
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
    println!("  -D <key=val>         Define a variable for conditional compilation");
    println!(
        "  -M <name=path>       Map a module name to a physical directory (e.g., -M std=./library/std)"
    );
    println!("  -I <name=path>       Map a module name to an imported kmeta root");
    println!("  --emit-kmeta <dir>   Emit a kmeta module snapshot directory");
    println!("  --root-module <name> Override the compiled root module name");
    println!("  -O<0-3>              Set optimization level (default: O0)");

    println!("\nTargeting & Codegen:");
    println!("  --target <T>         Set target triple (e.g. x86_64-unknown-linux-gnu)");
    println!("  --asm-dialect <D>    Set assembly dialect: intel (default) or att");
    println!("  --cc <cmd>           Set the linker driver command (default: $CC or cc)");
    println!("  --linker <cmd>       Alias for --cc");
    println!("  --runtime-entry <m>  Runtime entry contract: none, rt, crt");
    println!("  --runtime-provider <p>");
    println!("                       Runtime/platform provider: none, toolchain, libc");
    println!("  --runtime-libc <b>   Whether libc is linked: yes, no");
    println!("  --library-bundle <b> Library bundle: none, base, std");
    println!("  --link-input <path>  Add an extra linker input (.o/.a/.so/response file)");
    println!("  -L <dir>             Add a linker search path");
    println!("  -l <name>            Link against a library");
    println!("  --link-arg <arg>     Pass a raw argument through to the linker driver");
    println!("  --entry <symbol>     Override the default entry symbol used by kernc");
    println!("  --print-link-command Print the resolved linker command before execution");
    println!("  --emit-llvm          Print LLVM IR to stdout");
    println!("  --time               Print compiler phase timings");

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

fn parse_runtime_provider(value: &str) -> RuntimeProvider {
    RuntimeProvider::parse(value).unwrap_or_else(|err| cli_error(err))
}

fn parse_library_bundle(value: &str) -> LibraryBundle {
    LibraryBundle::parse(value).unwrap_or_else(|err| cli_error(err))
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

    validate_runtime_options(options).unwrap_or_else(|err| cli_error(err));
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
            "--emit-kmeta" => {
                options.metadata_output =
                    Some(next_option_value(&mut args, "--emit-kmeta", "directory"));
            }
            "--root-module" => {
                options.root_module_name =
                    Some(next_option_value(&mut args, "--root-module", "module name"));
            }
            "--time" => options.report_timings = true,
            "--target" => {
                let triple = next_option_value(&mut args, "--target", "target triple");
                options.target = parse_target_machine(&triple);
            }
            "--asm-dialect" => {
                let dialect = next_option_value(&mut args, "--asm-dialect", "dialect");
                options.asm_dialect = parse_asm_dialect(&dialect);
            }
            "--cc" | "--linker" => {
                options.linker_cmd = next_option_value(&mut args, arg.as_str(), "command")
            }
            "--runtime-entry" => {
                let mode = next_option_value(&mut args, "--runtime-entry", "mode");
                options.runtime_entry = parse_runtime_entry(&mode);
            }
            "--runtime-provider" => {
                let provider = next_option_value(&mut args, "--runtime-provider", "provider");
                options.runtime_provider = parse_runtime_provider(&provider);
            }
            "--runtime-libc" => {
                let enabled = next_option_value(&mut args, "--runtime-libc", "yes|no");
                options.runtime_libc = parse_yes_no(&enabled, "--runtime-libc");
            }
            "--library-bundle" => {
                let bundle = next_option_value(&mut args, "--library-bundle", "bundle");
                options.library_bundle = parse_library_bundle(&bundle);
            }
            "--link-input" => {
                options
                    .linker_inputs
                    .push(next_option_value(&mut args, "--link-input", "path"))
            }
            "--link-arg" => {
                options
                    .linker_args
                    .push(next_option_value(&mut args, "--link-arg", "argument"))
            }
            "--entry" => {
                options.entry_symbol = Some(next_option_value(&mut args, "--entry", "symbol"));
            }
            "--print-link-command" => options.print_link_command = true,
            "-D" => {
                let define = next_option_value(&mut args, "-D", "`key=value`");
                let (key, value) = parse_key_value(define, "-D", "key=value");
                options.custom_defines.insert(key, value);
            }
            "-M" => {
                let mapping = next_option_value(&mut args, "-M", "`name=path`");
                let (name, path) = parse_key_value(mapping, "-M", "name=path");
                options.module_aliases.insert(name, path);
            }
            "-I" => {
                let mapping = next_option_value(&mut args, "-I", "`name=path`");
                let (name, path) = parse_key_value(mapping, "-I", "name=path");
                options.module_interface_aliases.insert(name, path);
            }
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
    inject_default_library_aliases(&mut options);

    options
}

fn main() {
    let options = parse_args();
    let driver = CompilerDriver::new(options);

    if !driver.compile() {
        process::exit(1);
    }
}
