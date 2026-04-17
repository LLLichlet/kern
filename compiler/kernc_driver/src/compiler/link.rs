use super::{CompilerDriver, LinkTarget, TempFileGuard};
use kernc_utils::config::{RuntimeEntry, runtime_links_libc, runtime_uses_crt_startup};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const ARCHIVE_MAGIC: &[u8] = b"!<arch>\n";

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
                self.maybe_print_lto_toolchain_hint(target);
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

    pub(super) fn make_multi_linker_input_dir_path(&self) -> String {
        format!("{}.d", self.options.output_file)
    }

    pub(super) fn make_multi_linker_input_codegen_unit_path(&self, unit_name: &str) -> String {
        PathBuf::from(self.make_multi_linker_input_dir_path())
            .join(format!("{unit_name}.o"))
            .to_string_lossy()
            .to_string()
    }

    pub(super) fn make_temp_relocatable_merge_path(&self) -> String {
        format!("{}.tmp.merge.o", self.options.output_file)
    }

    fn resolve_linker_driver(&self, is_windows: bool) -> String {
        let requests_llvm_lto = self
            .options
            .linker_args
            .iter()
            .any(|arg| arg.starts_with("-flto"));
        if self.options.linker_cmd == "cc" && (is_windows || requests_llvm_lto) {
            find_llvm_tool("clang", is_windows).unwrap_or_else(|| "clang".to_string())
        } else {
            self.options.linker_cmd.clone()
        }
    }

    fn build_link_command(&self, extra_inputs: &[String], target: &LinkTarget) -> Command {
        let cc_compiler = self.resolve_linker_driver(target.is_windows);
        let mut cmd = Command::new(&cc_compiler);
        let requests_llvm_lto = self
            .options
            .linker_args
            .iter()
            .any(|arg| arg.starts_with("-flto"));
        let explicit_fuse_ld = self
            .options
            .linker_args
            .iter()
            .any(|arg| arg.starts_with("-fuse-ld="));
        let auto_use_lld = requests_llvm_lto
            && !explicit_fuse_ld
            && !target.is_darwin
            && (target.is_windows || find_llvm_tool("ld.lld", target.is_windows).is_some());

        if requests_llvm_lto && let Some(bin_dir) = llvm_prefix_bin_dir() {
            prepend_path_env(&mut cmd, &bin_dir);
        }

        if auto_use_lld {
            // LLVM bitcode inputs should stay on the same LLVM linker family
            // that produced them when an in-prefix lld is available.
            cmd.arg("-fuse-ld=lld");
        }

        for input in extra_inputs {
            Self::push_link_input_arg(&mut cmd, input, target.is_windows);
        }

        for input in &self.options.linker_inputs {
            Self::push_link_input_arg(&mut cmd, input, target.is_windows);
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

        self.apply_thin_lto_cache_options(&mut cmd, target, auto_use_lld);
        self.apply_dead_strip_options(&mut cmd, target.is_windows, target.is_darwin);

        cmd
    }

    fn push_link_input_arg(cmd: &mut Command, input: &str, is_windows: bool) {
        if is_windows && Self::is_archive_link_input(input) && !input.ends_with(".lib") {
            cmd.arg(format!("-Wl,/wholearchive:{input}"));
        } else {
            cmd.arg(input);
        }
    }

    fn is_archive_link_input(input: &str) -> bool {
        let Ok(bytes) = fs::read(input) else {
            return false;
        };
        bytes.starts_with(ARCHIVE_MAGIC)
    }

    fn build_relocatable_link_command(
        &self,
        inputs: &[String],
        target: &LinkTarget,
        output_path: &str,
    ) -> Command {
        let mut cmd = if target.is_windows {
            Command::new("llvm-lib")
        } else {
            let cc_compiler = self.resolve_linker_driver(target.is_windows);
            let mut cmd = Command::new(&cc_compiler);
            cmd.arg("-r");
            cmd
        };
        for input in inputs {
            cmd.arg(input);
        }
        if target.is_windows {
            cmd.arg(format!("/out:{output_path}"));
        } else {
            cmd.arg("-o").arg(output_path);
        }
        cmd
    }

    fn apply_runtime_contract(&self, cmd: &mut Command, is_windows: bool, is_darwin: bool) {
        if is_windows && !matches!(self.options.runtime_entry, RuntimeEntry::None) {
            cmd.arg("-lshell32");
        }

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
                        cmd.arg("-Wl,/subsystem:console");
                        cmd.arg("-lkernel32");
                        let entry_symbol = self
                            .options
                            .entry_symbol
                            .as_deref()
                            .unwrap_or("mainCRTStartup");
                        cmd.arg(format!("-Wl,/entry:{}", entry_symbol));
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

    fn apply_thin_lto_cache_options(
        &self,
        cmd: &mut Command,
        target: &LinkTarget,
        auto_use_lld: bool,
    ) {
        if target.is_windows
            || !self
                .options
                .linker_args
                .iter()
                .any(|arg| arg == "-flto=thin")
            || self.options.linker_args.iter().any(|arg| {
                arg.contains("thinlto-cache-dir")
                    || arg.contains("cache_path_lto")
                    || arg.contains("plugin-opt,cache-dir=")
            })
        {
            return;
        }

        let cache_dir = self.make_thin_lto_cache_dir_path();
        let cache_dir_path = PathBuf::from(&cache_dir);
        if cache_dir_path.is_file() && fs::remove_file(&cache_dir_path).is_err() {
            eprintln!(
                "Warning: failed to remove stale ThinLTO cache file `{}`; continuing without ThinLTO cache",
                cache_dir_path.display()
            );
            return;
        }
        if let Err(err) = fs::create_dir_all(&cache_dir_path) {
            eprintln!(
                "Warning: failed to create ThinLTO cache dir `{}`: {}; continuing without ThinLTO cache",
                cache_dir_path.display(),
                err
            );
            return;
        }

        let uses_lld = auto_use_lld
            || self
                .options
                .linker_args
                .iter()
                .any(|arg| arg == "-fuse-ld=lld" || arg == "-fuse-ld=ld64.lld");

        if target.is_darwin {
            cmd.arg(format!("-Wl,-cache_path_lto,{}", cache_dir));
        } else if uses_lld {
            cmd.arg(format!("-Wl,--thinlto-cache-dir={}", cache_dir));
        } else {
            cmd.arg(format!("-Wl,-plugin-opt,cache-dir={}", cache_dir));
        }
    }

    fn maybe_print_link_command(&self, cmd: &Command) {
        if self.options.print_link_command {
            println!("Link command: {}", self.format_command(cmd));
        }
    }

    fn maybe_print_lto_toolchain_hint(&self, target: &LinkTarget) {
        let requests_llvm_lto = self
            .options
            .linker_args
            .iter()
            .any(|arg| arg.starts_with("-flto"));
        if !requests_llvm_lto || target.is_windows {
            return;
        }

        let llvm_prefix = llvm_prefix_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unset>".to_string());
        eprintln!(
            "Note: LLVM LTO links require the final linker toolchain to match the LLVM version used for codegen."
        );
        eprintln!(
            "      If you see Producer/Reader version mismatches, use a matching clang/lld pair via PATH or CC."
        );
        eprintln!(
            "      Current runtime LLVM_SYS prefix: {}",
            llvm_prefix
        );
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

fn llvm_prefix_dir() -> Option<PathBuf> {
    env::vars().find_map(|(key, value)| {
        (key.starts_with("LLVM_SYS_") && key.ends_with("_PREFIX")).then(|| PathBuf::from(value))
    })
}

fn llvm_prefix_bin_dir() -> Option<PathBuf> {
    let bin_dir = llvm_prefix_dir()?.join("bin");
    bin_dir.is_dir().then_some(bin_dir)
}

fn find_llvm_tool(tool: &str, is_windows: bool) -> Option<String> {
    let prefix = llvm_prefix_dir()?;
    find_llvm_tool_in_prefix(&prefix, tool, is_windows)
}

fn find_llvm_tool_in_prefix(prefix: &std::path::Path, tool: &str, is_windows: bool) -> Option<String> {
    let bin_dir = prefix.join("bin");
    let tool_name = if is_windows {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    };
    let direct = bin_dir.join(&tool_name);
    if direct.is_file() {
        return Some(direct.to_string_lossy().to_string());
    }

    let suffix = prefix
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("llvm-"))?;
    let versioned = if is_windows {
        bin_dir.join(format!("{tool}-{suffix}.exe"))
    } else {
        bin_dir.join(format!("{tool}-{suffix}"))
    };
    versioned
        .is_file()
        .then(|| versioned.to_string_lossy().to_string())
}

fn prepend_path_env(cmd: &mut Command, dir: &std::path::Path) {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    if let Ok(joined) = env::join_paths(paths) {
        cmd.env("PATH", joined);
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

#[cfg(test)]
mod tests {
    use super::*;
    use kernc_utils::config::CompileOptions;

    fn command_args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn thin_lto_links_add_cache_dir_by_default() {
        let root = std::env::temp_dir().join(format!(
            "kern_link_thinlto_cache_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("main.out");
        let driver = CompilerDriver::new(CompileOptions {
            output_file: output.to_string_lossy().to_string(),
            linker_args: vec!["-flto=thin".to_string()],
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();

        let cmd = driver.build_link_command(&[], &target);
        let args = command_args(&cmd);
        let cache_dir = format!("{}.thinlto-cache.d", output.to_string_lossy());

        if target.is_darwin {
            assert!(
                args.iter()
                    .any(|arg| arg == &format!("-Wl,-cache_path_lto,{}", cache_dir))
            );
        } else if !target.is_windows {
            if find_llvm_tool("ld.lld", false).is_some() {
                assert!(
                    args.iter()
                        .any(|arg| arg == &format!("-Wl,--thinlto-cache-dir={}", cache_dir))
                );
                assert!(args.iter().any(|arg| arg == "-fuse-ld=lld"));
            } else {
                assert!(
                    args.iter()
                        .any(|arg| arg == &format!("-Wl,-plugin-opt,cache-dir={}", cache_dir))
                );
            }
        }
        if target.is_windows {
            assert!(!PathBuf::from(&cache_dir).is_dir());
        } else {
            assert!(PathBuf::from(&cache_dir).is_dir());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn llvm_tool_lookup_prefers_prefix_bin_tools() {
        let root = std::env::temp_dir().join(format!(
            "kern_link_llvm_tool_lookup_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("clang"), "").unwrap();
        fs::write(bin_dir.join("ld.lld"), "").unwrap();

        let clang = find_llvm_tool_in_prefix(&root, "clang", false);
        let lld = find_llvm_tool_in_prefix(&root, "ld.lld", false);

        assert_eq!(
            clang.as_deref(),
            Some(bin_dir.join("clang").to_string_lossy().as_ref())
        );
        assert_eq!(
            lld.as_deref(),
            Some(bin_dir.join("ld.lld").to_string_lossy().as_ref())
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn thin_lto_links_respect_explicit_cache_dir_flags() {
        let root = std::env::temp_dir().join(format!(
            "kern_link_thinlto_cache_explicit_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("main.out");
        let explicit = root.join("custom-cache");
        let explicit_flag = format!("-Wl,-plugin-opt,cache-dir={}", explicit.to_string_lossy());
        let driver = CompilerDriver::new(CompileOptions {
            output_file: output.to_string_lossy().to_string(),
            linker_args: vec!["-flto=thin".to_string(), explicit_flag.clone()],
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();

        let cmd = driver.build_link_command(&[], &target);
        let args = command_args(&cmd);

        assert_eq!(
            args.iter()
                .filter(|arg| {
                    arg.contains("thinlto-cache-dir")
                        || arg.contains("cache_path_lto")
                        || arg.contains("plugin-opt,cache-dir=")
                })
                .count(),
            1
        );
        assert!(args.iter().any(|arg| arg == &explicit_flag));

        let _ = fs::remove_dir_all(&root);
    }
}
