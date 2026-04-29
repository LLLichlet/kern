mod help;

use help::{HelpTopic, render_help, version_text};
use kernc_driver::CompilerDriver;
use kernc_utils::config::{
    AsmDialect, CodeModel, CompileOptions, DriverMode, LibraryBundle, LlvmIrStage, LtoMode,
    OptLevel, RuntimeEntry, TargetMachine, apply_configured_library_aliases,
    inject_driver_condition_defines, validate_compile_options,
};
use shared_cli::{ColorChoice, ErrorReport};
use std::env;
use std::path::Path;
use std::process;

fn set_driver_mode(options: &mut CompileOptions, requested: DriverMode, flag: &str) {
    if options.driver_mode != DriverMode::CompileAndLink && options.driver_mode != requested {
        cli_error(format!(
            "`{}` conflicts with a previously selected driver mode.",
            flag
        ));
    }
    options.driver_mode = requested;
}

enum CliAction {
    Run(Box<CompileOptions>),
    Help(HelpTopic),
    Version,
}

fn cli_error(message: impl Into<String>) -> ! {
    eprint!(
        "{}",
        ErrorReport::new("kernc error", message.into()).render(ColorChoice::Auto)
    );
    process::exit(1);
}

fn cli_error_with_hint(message: impl Into<String>, hint: impl Into<String>) -> ! {
    eprint!(
        "{}",
        ErrorReport::new("kernc error", message.into())
            .hint(hint.into())
            .render(ColorChoice::Auto)
    );
    process::exit(1);
}

fn next_option_value(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
    value_name: &str,
) -> String {
    args.next()
        .unwrap_or_else(|| cli_error(format!("Expected {} after `{}`.", value_name, flag)))
}

fn parse_target_machine(value: &str) -> TargetMachine {
    TargetMachine::new(value).unwrap_or_else(|e| cli_error(format!("Invalid target triple: {}", e)))
}

fn parse_asm_dialect(value: &str) -> AsmDialect {
    match value {
        "auto" => AsmDialect::Auto,
        "intel" => AsmDialect::Intel,
        "att" => AsmDialect::Att,
        _ => cli_error(format!(
            "Invalid asm dialect `{}`. Expected one of: auto, intel, att.",
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

fn parse_code_model(value: &str) -> CodeModel {
    CodeModel::parse(value).unwrap_or_else(|err| cli_error(err))
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
    args: &mut impl Iterator<Item = String>,
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
    args: &mut impl Iterator<Item = String>,
    value_name: &str,
) -> Option<String> {
    if arg == flag {
        return Some(next_option_value(args, flag, value_name));
    }

    let prefix = format!("{flag}=");
    arg.strip_prefix(&prefix).map(|value| value.to_string())
}

fn default_executable_output_name(input_file: Option<&str>) -> String {
    let stem = input_file
        .and_then(|input| Path::new(input).file_stem())
        .and_then(|s| s.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("a");
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

fn apply_cli_runtime_defaults(
    options: &mut CompileOptions,
    user_selected_runtime_entry: bool,
    user_selected_library_bundle: bool,
) {
    if options.driver_mode != DriverMode::CompileAndLink {
        return;
    }

    if !user_selected_runtime_entry {
        options.runtime_entry = RuntimeEntry::Rt;
    }
    if !user_selected_library_bundle {
        options.library_bundle = LibraryBundle::Std;
    }
}

fn set_default_output_file(options: &mut CompileOptions) {
    if !options.output_file.is_empty() {
        return;
    }

    match options.driver_mode {
        DriverMode::CompileOnly | DriverMode::CcCompile => {
            let stem = options
                .input_file
                .as_deref()
                .and_then(|input| Path::new(input).file_stem())
                .and_then(|s| s.to_str())
                .unwrap_or("a.out");
            options.output_file = format!("{}.o", stem);
        }
        DriverMode::CompileAndLink => {
            options.output_file = default_executable_output_name(options.input_file.as_deref());
        }
        _ => {
            options.output_file = format!("a{}", std::env::consts::EXE_SUFFIX);
        }
    }
}

fn validate_mode_inputs(
    program_name: &str,
    options: &CompileOptions,
    positional_source: &Option<String>,
) {
    if options.driver_mode.needs_source_input() && positional_source.is_none() {
        cli_error_with_hint(
            "No input file specified.",
            format!(
                "Run `{program_name} --help` for the common view or `{program_name} help all` for the full option reference."
            ),
        );
    }

    if options.driver_mode == DriverMode::LinkOnly && positional_source.is_some() {
        cli_error_with_hint(
            "`--link-only` does not accept a source input.",
            "Pass object files, archives, or shared libraries via `--link-input`.",
        );
    }

    validate_compile_options(options).unwrap_or_else(|err| cli_error(err));
}

fn parse_help_request(args: &[String]) -> Option<CliAction> {
    let first = args.first()?;
    if args.iter().any(|arg| arg == "--help=all") {
        return Some(CliAction::Help(HelpTopic::All));
    }
    if first == "help" {
        return match args.get(1).map(String::as_str) {
            None => Some(CliAction::Help(HelpTopic::Overview)),
            Some("all") => Some(CliAction::Help(HelpTopic::All)),
            Some(other) => {
                cli_error_with_hint(
                    format!("Unknown help topic `{other}`. Expected `all` or no topic."),
                    "Run `kernc help all` for the full option reference.",
                );
            }
        };
    }
    if first == "--help" || first == "-h" || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Some(CliAction::Help(HelpTopic::Overview));
    }
    None
}

fn parse_version_request(args: &[String]) -> Option<CliAction> {
    let first = args.first()?;
    if first == "--version" || first == "-V" || (first == "-v" && args.len() == 1) {
        return Some(CliAction::Version);
    }
    if args
        .iter()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        return Some(CliAction::Version);
    }
    None
}

fn parse_args() -> CliAction {
    let mut args = env::args();
    let program_name = args.next().unwrap_or_else(|| "kernc".to_string());
    let raw_args: Vec<String> = args.collect();

    if let Some(action) = parse_version_request(&raw_args) {
        return action;
    }
    if let Some(action) = parse_help_request(&raw_args) {
        return action;
    }
    let mut args = raw_args.into_iter();

    let mut options = CompileOptions::default();
    options.output_file.clear();
    let mut user_selected_runtime_entry = false;
    let mut user_selected_library_bundle = false;

    // Read environment variables before parsing CLI arguments.
    if let Ok(toolchain_root) = env::var("KERN_TOOLCHAIN_ROOT")
        && !toolchain_root.is_empty()
    {
        options.toolchain_root = Some(toolchain_root);
    }
    if let Ok(cc_env) = env::var("CC")
        && !cc_env.is_empty()
    {
        options.linker_cmd = cc_env;
        options.linker_cmd_explicit = true;
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
        if let Some(value) = consume_long_option_value(&arg, "--code-model", &mut args, "model") {
            options.code_model = parse_code_model(&value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--debug-info", &mut args, "yes|no") {
            options.debug_info = parse_yes_no(&value, "--debug-info");
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--link-driver", &mut args, "command")
        {
            options.linker_cmd = value;
            options.linker_cmd_explicit = true;
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--toolchain-root", &mut args, "directory")
        {
            options.toolchain_root = Some(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--emit-llvm=") {
            set_driver_mode(&mut options, DriverMode::EmitLlvmIr, "--emit-llvm");
            options.emit_llvm_stage = parse_llvm_ir_stage(value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--runtime-entry", &mut args, "mode") {
            user_selected_runtime_entry = true;
            options.runtime_entry = parse_runtime_entry(&value);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--runtime-libc", &mut args, "yes|no")
        {
            options.runtime_libc = parse_yes_no(&value, "--runtime-libc");
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--library-bundle", &mut args, "bundle")
        {
            user_selected_library_bundle = true;
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
        if let Some(value) = consume_long_option_value(&arg, "--cc-arg", &mut args, "argument") {
            options.cc_args.push(value);
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
            "-o" => options.output_file = next_option_value(&mut args, "-o", "file name"),
            "-c" => set_driver_mode(&mut options, DriverMode::CompileOnly, "-c"),
            "--cc" => set_driver_mode(&mut options, DriverMode::CcCompile, "--cc"),
            "--link-only" => set_driver_mode(&mut options, DriverMode::LinkOnly, "--link-only"),
            "-O0" => options.opt_level = OptLevel::O0,
            "-O1" => options.opt_level = OptLevel::O1,
            "-O2" => options.opt_level = OptLevel::O2,
            "-O3" => options.opt_level = OptLevel::O3,
            "-g" => options.debug_info = true,
            "-g0" => options.debug_info = false,
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
    apply_cli_runtime_defaults(
        &mut options,
        user_selected_runtime_entry,
        user_selected_library_bundle,
    );
    set_default_output_file(&mut options);
    inject_driver_condition_defines(&mut options);
    apply_configured_library_aliases(&mut options);

    CliAction::Run(Box::new(options))
}

fn main() {
    kernc_utils::install_compiler_panic_hook("kernc");

    match parse_args() {
        CliAction::Run(options) => {
            let driver = CompilerDriver::new(*options);
            if !driver.compile() {
                process::exit(1);
            }
        }
        CliAction::Help(topic) => {
            print!("{}", render_help("kernc", topic, ColorChoice::Auto));
        }
        CliAction::Version => {
            println!("{}", version_text());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CliAction, apply_cli_runtime_defaults, default_executable_output_name, parse_help_request,
        parse_version_request, set_default_output_file,
    };
    use crate::help::HelpTopic;
    use kernc_utils::config::{CompileOptions, DriverMode, LibraryBundle, RuntimeEntry};

    #[test]
    fn parses_overview_help_requests() {
        assert!(matches!(
            parse_help_request(&["--help".to_string()]),
            Some(CliAction::Help(HelpTopic::Overview))
        ));
        assert!(matches!(
            parse_help_request(&["hello.rn".to_string(), "--help".to_string()]),
            Some(CliAction::Help(HelpTopic::Overview))
        ));
    }

    #[test]
    fn parses_full_help_topic() {
        assert!(matches!(
            parse_help_request(&["help".to_string(), "all".to_string()]),
            Some(CliAction::Help(HelpTopic::All))
        ));
        assert!(matches!(
            parse_help_request(&["--help=all".to_string()]),
            Some(CliAction::Help(HelpTopic::All))
        ));
    }

    #[test]
    fn parses_version_requests() {
        assert!(matches!(
            parse_version_request(&["--version".to_string()]),
            Some(CliAction::Version)
        ));
        assert!(matches!(
            parse_version_request(&["-V".to_string()]),
            Some(CliAction::Version)
        ));
    }

    #[test]
    fn applies_direct_build_runtime_defaults_only_when_unspecified() {
        let mut options = CompileOptions {
            driver_mode: DriverMode::CompileAndLink,
            ..CompileOptions::default()
        };

        apply_cli_runtime_defaults(&mut options, false, false);
        assert_eq!(options.runtime_entry, RuntimeEntry::Rt);
        assert_eq!(options.library_bundle, LibraryBundle::Std);

        apply_cli_runtime_defaults(&mut options, true, true);
        assert_eq!(options.runtime_entry, RuntimeEntry::Rt);
        assert_eq!(options.library_bundle, LibraryBundle::Std);

        let mut compile_only = CompileOptions {
            driver_mode: DriverMode::CompileOnly,
            ..CompileOptions::default()
        };
        apply_cli_runtime_defaults(&mut compile_only, false, false);
        assert_eq!(compile_only.runtime_entry, RuntimeEntry::None);
        assert_eq!(compile_only.library_bundle, LibraryBundle::None);
    }

    #[test]
    fn defaults_linked_output_name_to_source_stem() {
        assert_eq!(
            default_executable_output_name(Some("examples/hello_world.rn")),
            format!("hello_world{}", std::env::consts::EXE_SUFFIX)
        );

        let mut options = CompileOptions {
            input_file: Some("examples/hello_world.rn".to_string()),
            output_file: String::new(),
            driver_mode: DriverMode::CompileAndLink,
            ..CompileOptions::default()
        };
        set_default_output_file(&mut options);
        assert_eq!(
            options.output_file,
            format!("hello_world{}", std::env::consts::EXE_SUFFIX)
        );
    }

    #[test]
    fn defaults_cc_output_name_to_object_stem() {
        let mut options = CompileOptions {
            input_file: Some("native/demo.c".to_string()),
            output_file: String::new(),
            driver_mode: DriverMode::CcCompile,
            ..CompileOptions::default()
        };
        set_default_output_file(&mut options);
        assert_eq!(options.output_file, "demo.o");
    }
}
