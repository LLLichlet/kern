use kernc_driver::CompilerDriver;
use kernc_utils::config::{
    AsmDialect, CompileOptions, DriverMode, LinkProfile, OptLevel, TargetMachine,
};
use std::env;
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
    println!("Usage: {} [OPTIONS] [input.kr]\n", program_name);

    println!("Build Options:");
    println!("  -o <file>            Write output to <file>");
    println!("  -c                   Emit linker input and skip the final system link step");
    println!("  --link-only          Skip frontend/codegen and invoke the linker driver only");
    println!("  -D <key=val>         Define a variable for conditional compilation");
    println!(
        "  -M <name=path>       Map a module name to a physical directory (e.g., -M std=./library/std)"
    );
    println!("  -O<0-3>              Set optimization level (default: O0)");

    println!("\nTargeting & Codegen:");
    println!("  --target <T>         Set target triple (e.g. x86_64-unknown-linux-gnu)");
    println!("  --asm-dialect <D>    Set assembly dialect: intel (default) or att");
    println!("  --cc <cmd>           Set the linker driver command (default: $CC or cc)");
    println!("  --linker <cmd>       Alias for --cc");
    println!("  --link-profile <p>   Default link policy: kern, freestanding, hosted, none");
    println!("  --link-input <path>  Add an extra linker input (.o/.a/.so/response file)");
    println!("  -L <dir>             Add a linker search path");
    println!("  -l <name>            Link against a library");
    println!("  --link-arg <arg>     Pass a raw argument through to the linker driver");
    println!("  --entry <symbol>     Override the default entry symbol used by kernc");
    println!("  --print-link-command Print the resolved linker command before execution");
    println!("  --no-default-link-flags");
    println!("                       Alias for `--link-profile none`");
    println!("  --link-libc          Alias for `--link-profile hosted`");
    println!(
        "  --use-std            Enable the Kern standard library (mutually exclusive with --link-libc)"
    );
    println!("  --emit-llvm          Print LLVM IR to stdout");

    println!("\nInformation:");
    println!("  -v, --version        Display version information and exit");
    println!("  -h, --help           Display this help and exit");
}

fn parse_args() -> CompileOptions {
    let mut args = env::args();
    let program_name = args.next().unwrap_or_else(|| "kernc".to_string());

    let mut options = CompileOptions::default();
    let mut output_file_set = false;

    // 在解析 CLI 参数之前读取环境变量。
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
            "-o" => {
                options.output_file = args.next().expect("Expected file name after `-o`");
                output_file_set = true;
            }
            "-c" => set_driver_mode(&mut options, DriverMode::CompileOnly, "-c"),
            "--link-only" => set_driver_mode(&mut options, DriverMode::LinkOnly, "--link-only"),
            "-O0" => options.opt_level = OptLevel::O0,
            "-O1" => options.opt_level = OptLevel::O1,
            "-O2" => options.opt_level = OptLevel::O2,
            "-O3" => options.opt_level = OptLevel::O3,
            "--emit-llvm" => set_driver_mode(&mut options, DriverMode::EmitLlvmIr, "--emit-llvm"),
            "--link-libc" => options.link_profile = LinkProfile::Hosted,
            "--use-std" => options.use_std = true,
            "--target" => {
                let triple_str = args
                    .next()
                    .expect("Expected target triple after `--target`");
                options.target = TargetMachine::new(&triple_str).unwrap_or_else(|e| {
                    eprintln!("Error: Invalid target triple: {}", e);
                    process::exit(1);
                });
            }
            "--asm-dialect" => {
                let dialect_str = args.next().expect("Expected dialect after `--asm-dialect`");
                options.asm_dialect = match dialect_str.as_str() {
                    "intel" => AsmDialect::Intel,
                    "att" => AsmDialect::Att,
                    _ => {
                        eprintln!("Error: Invalid asm dialect");
                        process::exit(1);
                    }
                };
            }
            "--cc" | "--linker" => {
                options.linker_cmd = args.next().expect("Expected command after `--cc`");
            }
            "--link-profile" => {
                let profile = args
                    .next()
                    .expect("Expected profile after `--link-profile`");
                options.link_profile = match profile.as_str() {
                    "kern" => LinkProfile::Kern,
                    "freestanding" => LinkProfile::Freestanding,
                    "hosted" => LinkProfile::Hosted,
                    "none" => LinkProfile::None,
                    _ => {
                        eprintln!(
                            "Error: Invalid link profile `{}`. Expected one of: kern, freestanding, hosted, none.",
                            profile
                        );
                        process::exit(1);
                    }
                };
            }
            "--link-input" => {
                options
                    .linker_inputs
                    .push(args.next().expect("Expected path after `--link-input`"));
            }
            "--link-arg" => {
                options
                    .linker_args
                    .push(args.next().expect("Expected argument after `--link-arg`"));
            }
            "--entry" => {
                options.entry_symbol = Some(args.next().expect("Expected symbol after `--entry`"));
            }
            "--print-link-command" => options.print_link_command = true,
            "--no-default-link-flags" => options.link_profile = LinkProfile::None,
            "-D" => {
                let define_str = args.next().expect("Expected `key=value` after `-D`");
                let parts: Vec<&str> = define_str.splitn(2, '=').collect();
                if parts.len() != 2 {
                    eprintln!("Error: Invalid define format. Expected `key=value`.");
                    process::exit(1);
                }
                options
                    .custom_defines
                    .insert(parts[0].to_string(), parts[1].to_string());
            }
            "-M" => {
                let map_str = args.next().expect("Expected `name=path` after `-M`");
                let parts: Vec<&str> = map_str.splitn(2, '=').collect();
                if parts.len() != 2 {
                    eprintln!("Error: Invalid module map format. Expected `name=path`.");
                    process::exit(1);
                }
                options
                    .module_aliases
                    .insert(parts[0].to_string(), parts[1].to_string());
            }
            _ => {
                if let Some(path) = arg.strip_prefix("-L") {
                    if path.is_empty() {
                        options
                            .linker_search_paths
                            .push(args.next().expect("Expected path after `-L`"));
                    } else {
                        options.linker_search_paths.push(path.to_string());
                    }
                    continue;
                }

                if let Some(lib) = arg.strip_prefix("-l") {
                    if lib.is_empty() {
                        options
                            .linker_libraries
                            .push(args.next().expect("Expected library name after `-l`"));
                    } else {
                        options.linker_libraries.push(lib.to_string());
                    }
                    continue;
                }

                if arg.starts_with('-') {
                    eprintln!("Error: Unrecognized option `{}`", arg);
                    process::exit(1);
                }
                if positional_source.is_some() {
                    eprintln!("Error: Multiple input files are not supported yet.");
                    process::exit(1);
                }
                positional_source = Some(arg);
            }
        }
    }

    if options.driver_mode.needs_source_input() && positional_source.is_none() {
        eprintln!("Error: No input file specified.");
        print_usage(&program_name);
        process::exit(1);
    }

    options.input_file = positional_source;

    if options.driver_mode == DriverMode::LinkOnly && options.input_file.is_some() {
        eprintln!("Error: `--link-only` does not accept a source input.");
        eprintln!("Hint: Pass object files, archives, or shared libraries via `--link-input`.");
        process::exit(1);
    }

    if !output_file_set {
        match options.driver_mode {
            DriverMode::CompileOnly => {
                let ext = if options.target.triple.to_string().contains("windows") {
                    "ll"
                } else {
                    "o"
                };
                let stem = options
                    .input_file
                    .as_deref()
                    .and_then(|input| std::path::Path::new(input).file_stem())
                    .and_then(|s| s.to_str())
                    .unwrap_or("a.out");
                options.output_file = format!("{}.{}", stem, ext);
            }
            _ => {
                options.output_file = "a.out".to_string();
            }
        }
    }

    // Kern Std 与 C Libc 严格互斥
    if options.use_std && options.link_profile == LinkProfile::Hosted {
        eprintln!("Error: `--use-std` and `--link-libc` are strictly mutually exclusive.");
        eprintln!(
            "Hint: Kern enforces a strict separation between its native freestanding environment and the C hosted environment."
        );
        process::exit(1);
    }

    if options.use_std && !options.module_aliases.contains_key("std") {
        let std_path = if let Ok(custom_std) = env::var("KERN_STD_PATH") {
            std::path::PathBuf::from(custom_std)
        } else if let Ok(mut exe_path) = env::current_exe() {
            exe_path.pop(); // bin/
            if exe_path.ends_with("debug") || exe_path.ends_with("release") {
                exe_path.pop(); // debug/
                exe_path.pop(); // target/
                exe_path.join("library/std")
            } else {
                exe_path.pop(); // kern/
                exe_path.join("lib/kern/std")
            }
        } else {
            std::path::PathBuf::from("library/std")
        };

        if !std_path.exists() {
            eprintln!(
                "Warning: Kern standard library not found at `{}`.",
                std_path.display()
            );
        }

        options
            .module_aliases
            .insert("std".to_string(), std_path.to_string_lossy().to_string());
    }

    options
}

fn main() {
    let options = parse_args();
    let driver = CompilerDriver::new(options);

    if !driver.compile() {
        process::exit(1);
    }
}
