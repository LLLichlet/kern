use super::{CompilerDriver, LinkTarget, TempFileGuard};
use kernc_utils::config::{RuntimeEntry, runtime_links_libc, runtime_uses_crt_startup};
use std::env;
use std::process::Command;

impl CompilerDriver {
    pub(super) fn link_only(&self) -> bool {
        if self.options.linker_inputs.is_empty() {
            eprintln!("Error: `--link-only` requires at least one `--link-input`.");
            return false;
        }

        let target = self.normalized_target();
        self.run_link_command_with_inputs(&[], &target, "Successfully linked")
    }

    pub(super) fn normalized_target(&self) -> LinkTarget {
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

    pub(super) fn prepare_link_input_path(&self, _target: &LinkTarget) -> String {
        if self.options.driver_mode.emits_linker_input() {
            self.options.output_file.clone()
        } else {
            self.make_temp_link_input_path()
        }
    }

    pub(super) fn temp_link_input_guard(&self, link_input_path: &str) -> Option<TempFileGuard> {
        if self.options.driver_mode.emits_linker_input() {
            None
        } else {
            Some(TempFileGuard {
                path: link_input_path.to_string(),
            })
        }
    }

    pub(super) fn run_link_command(
        &self,
        link_input_path: Option<&str>,
        target: &LinkTarget,
        success_prefix: &str,
    ) -> bool {
        let extra_inputs = link_input_path
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        self.run_link_command_with_inputs(&extra_inputs, target, success_prefix)
    }

    pub(super) fn run_link_command_with_inputs(
        &self,
        extra_inputs: &[String],
        target: &LinkTarget,
        success_prefix: &str,
    ) -> bool {
        if self.options.report_progress {
            println!("Linking for target: {} ...", target.triple);
        }
        let mut cmd = self.build_link_command(extra_inputs, target);
        self.maybe_print_link_command(&cmd);

        match cmd.status() {
            Ok(status) if status.success() => {
                if self.options.report_progress {
                    println!("{} to `{}`", success_prefix, self.options.output_file);
                }
                true
            }
            Ok(status) => {
                eprintln!("Error: Linker failed with exit code {}", status);
                false
            }
            Err(err) => {
                let cc_compiler = self.resolve_linker_driver(target.is_windows);
                eprintln!(
                    "Error: Failed to invoke linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cc_compiler, err
                );
                false
            }
        }
    }

    pub(super) fn run_relocatable_link_command(
        &self,
        inputs: &[String],
        target: &LinkTarget,
        output_path: &str,
        display_output_path: &str,
        success_prefix: &str,
    ) -> bool {
        if inputs.is_empty() {
            eprintln!("Error: relocatable link requires at least one input object.");
            return false;
        }

        if target.is_windows {
            eprintln!(
                "Error: multi-object compile-only emission is not supported for Windows targets yet."
            );
            return false;
        }

        if self.options.report_progress {
            println!("Merging linker inputs for target: {} ...", target.triple);
        }
        let mut cmd = self.build_relocatable_link_command(inputs, target, output_path);
        self.maybe_print_link_command(&cmd);

        match cmd.status() {
            Ok(status) if status.success() => {
                if self.options.report_progress {
                    println!("{} to `{}`", success_prefix, display_output_path);
                }
                true
            }
            Ok(status) => {
                eprintln!("Error: Relocatable linker failed with exit code {}", status);
                false
            }
            Err(err) => {
                let cc_compiler = self.resolve_linker_driver(target.is_windows);
                eprintln!(
                    "Error: Failed to invoke relocatable linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cc_compiler, err
                );
                false
            }
        }
    }

    pub(super) fn make_temp_link_input_path(&self) -> String {
        format!("{}.tmp.o", self.options.output_file)
    }

    pub(super) fn make_temp_codegen_unit_path(&self, unit_name: &str) -> String {
        format!("{}.tmp.{}.o", self.options.output_file, unit_name)
    }

    pub(super) fn make_temp_relocatable_merge_path(&self) -> String {
        format!("{}.tmp.merge.o", self.options.output_file)
    }

    fn resolve_linker_driver(&self, is_windows: bool) -> String {
        if is_windows && self.options.linker_cmd == "cc" {
            "clang".to_string()
        } else {
            self.options.linker_cmd.clone()
        }
    }

    fn build_link_command(&self, extra_inputs: &[String], target: &LinkTarget) -> Command {
        let cc_compiler = self.resolve_linker_driver(target.is_windows);
        let mut cmd = Command::new(&cc_compiler);

        for input in extra_inputs {
            cmd.arg(input);
        }

        for input in &self.options.linker_inputs {
            cmd.arg(input);
        }

        cmd.arg("-o").arg(&self.options.output_file);

        self.apply_runtime_contract(&mut cmd, target.is_windows, target.is_darwin);

        for path in &self.options.linker_search_paths {
            cmd.arg(format!("-L{}", path));
        }

        for lib in &self.options.linker_libraries {
            cmd.arg(format!("-l{}", lib));
        }

        for arg in &self.options.linker_args {
            cmd.arg(arg);
        }

        self.apply_dead_strip_options(&mut cmd, target.is_windows, target.is_darwin);

        cmd
    }

    fn build_relocatable_link_command(
        &self,
        inputs: &[String],
        target: &LinkTarget,
        output_path: &str,
    ) -> Command {
        let cc_compiler = self.resolve_linker_driver(target.is_windows);
        let mut cmd = Command::new(&cc_compiler);
        cmd.arg("-r");
        for input in inputs {
            cmd.arg(input);
        }
        cmd.arg("-o").arg(output_path);
        cmd
    }

    fn apply_runtime_contract(&self, cmd: &mut Command, is_windows: bool, is_darwin: bool) {
        match self.options.runtime_entry {
            RuntimeEntry::None => {
                if let Some(entry_symbol) = &self.options.entry_symbol {
                    if is_windows {
                        cmd.arg("-Wl,/subsystem:console");
                        cmd.arg(format!("-Wl,/entry:{}", entry_symbol));
                    } else {
                        cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                    }
                }
            }
            RuntimeEntry::Crt => {
                if runtime_uses_crt_startup(&self.options) {
                    if is_windows {
                        if let Some(entry_symbol) = &self.options.entry_symbol {
                            cmd.arg(format!("-Wl,/entry:{}", entry_symbol));
                        }
                    } else if !is_darwin {
                        cmd.arg("-no-pie");
                        if let Some(entry_symbol) = &self.options.entry_symbol {
                            cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                        }
                    } else if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                    }
                }
            }
            RuntimeEntry::Rt => {
                if runtime_links_libc(&self.options) {
                    if is_darwin {
                        cmd.arg("-nostartfiles");
                        cmd.arg(format!(
                            "-Wl,-e,{}",
                            self.options.entry_symbol.as_deref().unwrap_or("_start")
                        ));
                    } else if is_windows {
                        cmd.arg("-lkernel32");
                        if let Some(entry_symbol) = &self.options.entry_symbol {
                            cmd.arg(format!("-Wl,/entry:{}", entry_symbol));
                        }
                    } else {
                        cmd.arg("-no-pie");
                        cmd.arg("-nostartfiles");
                        if let Some(entry_symbol) = &self.options.entry_symbol {
                            cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                        }
                    }
                } else if is_windows {
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

    fn apply_dead_strip_options(&self, cmd: &mut Command, is_windows: bool, is_darwin: bool) {
        if !self.options.dead_strip_sections {
            return;
        }

        if is_windows {
            cmd.arg("-Wl,/OPT:REF");
        } else if is_darwin {
            cmd.arg("-Wl,-dead_strip");
        } else {
            cmd.arg("-Wl,--gc-sections");
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

    if input.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '+' | '=' | ':')
    }) {
        return input.to_string();
    }

    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

fn normalize_darwin_triple_str(triple_str: &str) -> String {
    if triple_str.contains("macosx")
        && triple_str
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_digit())
    {
        return triple_str.to_string();
    }

    if triple_str.contains("darwin")
        && triple_str
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_digit())
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
