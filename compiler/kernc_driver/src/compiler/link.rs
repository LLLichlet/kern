use super::{CompilerDriver, LinkTarget, TempDirGuard, TempFileGuard};
use kernc_codegen::{
    ThinLtoModule, ThinLtoObject, ThinLtoObjectKind, ThinLtoOptions, run_thin_lto,
};
use kernc_utils::config::{OptLevel, RuntimeEntry, runtime_links_libc, runtime_uses_crt_startup};
use kernc_utils::llvm_bitcode::file_has_llvm_bitcode_magic;
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
        let mut prepared_driver = None;
        let mut prepared_thin_lto_output_dir = None;
        if self.should_materialize_thin_lto_link_inputs() {
            let Some((linker_inputs, output_dir_guard)) =
                self.materialize_thin_lto_link_inputs(&target)
            else {
                return false;
            };
            let mut options = self.options.clone();
            options.linker_inputs = linker_inputs;
            options.linker_args = self.linker_args_without_linker_driven_thin_lto();
            prepared_thin_lto_output_dir = Some(output_dir_guard);
            prepared_driver = Some(CompilerDriver::new(options));
        }

        let linked = prepared_driver
            .as_ref()
            .unwrap_or(self)
            .run_link_command_with_inputs(&[], &target, "Successfully linked");
        drop(prepared_thin_lto_output_dir);
        linked
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
        let mut cmd = match self.build_link_command(extra_inputs, target) {
            Ok(cmd) => cmd,
            Err(message) => {
                eprintln!("Error: {message}");
                return false;
            }
        };
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
                self.maybe_print_lto_toolchain_hint(target, &cmd);
                false
            }
            Err(err) => {
                eprintln!(
                    "Error: Failed to invoke linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cmd.get_program().to_string_lossy(),
                    err
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
        let mut cmd = match self.build_relocatable_link_command(inputs, target, output_path) {
            Ok(cmd) => cmd,
            Err(message) => {
                eprintln!("Error: {message}");
                return false;
            }
        };
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
                eprintln!(
                    "Error: Failed to invoke relocatable linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cmd.get_program().to_string_lossy(),
                    err
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

    fn resolve_linker_driver(&self, is_windows: bool) -> Result<String, String> {
        if self.options.linker_cmd_explicit {
            if self.options.linker_cmd.is_empty() {
                return Err(
                    "explicit linker/C driver command is empty; pass a non-empty --link-driver or CC"
                        .to_string(),
                );
            }
            return Ok(self.options.linker_cmd.clone());
        }

        if let Some(clang) = find_llvm_tool(&self.options, "clang", is_windows) {
            return Ok(clang);
        }

        if controlled_toolchain_root(&self.options).is_none()
            && let Some(host_driver) = find_host_c_driver(is_windows)
        {
            return Ok(host_driver);
        }

        Err(self.missing_default_clang_message(is_windows))
    }

    fn missing_default_clang_message(&self, is_windows: bool) -> String {
        let executable = if is_windows { "clang.exe" } else { "clang" };
        let searched_root = self.default_toolchain_root_hint();
        if controlled_toolchain_root(&self.options).is_none() {
            return format!(
                "No active Kern SDK/toolchain root or host C driver was found. Searched root: {searched_root}. Install the Kern SDK, set KERN_TOOLCHAIN_ROOT/--toolchain-root to a toolchain containing bin/{executable}, install host clang/cc for source-checkout development, or explicitly configure an external driver with --link-driver/CC."
            );
        }

        format!(
            "Kern SDK clang was not found. The default `cc` driver is SDK-owned when an SDK/toolchain root is active and does not fall back to host `cc`. Searched root: {searched_root}. Install or repair the Kern SDK, set KERN_TOOLCHAIN_ROOT/--toolchain-root to a toolchain containing bin/{executable}, or explicitly configure an external driver with --link-driver/CC."
        )
    }

    fn missing_default_llvm_tool_message(&self, tool: &str, is_windows: bool) -> String {
        let executable = if is_windows {
            format!("{tool}.exe")
        } else {
            tool.to_string()
        };
        let searched_root = self.default_toolchain_root_hint();
        format!(
            "Kern SDK {executable} was not found. Kern's default toolchain resolution is SDK-owned and does not fall back to host LLVM tools. Searched root: {searched_root}. Install or repair the Kern SDK, or set KERN_TOOLCHAIN_ROOT/--toolchain-root to a toolchain containing bin/{executable}."
        )
    }

    fn default_toolchain_root_hint(&self) -> String {
        if let Some(configured) = &self.options.toolchain_root {
            let root = PathBuf::from(configured);
            if root.is_dir() {
                root.display().to_string()
            } else {
                format!("{} (not a directory)", root.display())
            }
        } else {
            resolved_toolchain_root(&self.options)
                .map(|root| root.display().to_string())
                .unwrap_or_else(|| "<no active SDK/toolchain root>".to_string())
        }
    }

    pub(super) fn cc_compile_only(&self) -> bool {
        let Some(input_file) = self.options.input_file.as_deref() else {
            eprintln!("Error: `--cc` requires a C-family source input.");
            return false;
        };
        if self.options.output_file.is_empty() {
            eprintln!("Error: `--cc` requires an output object path.");
            return false;
        }

        let target = self.normalized_target();
        if let Some(parent) = std::path::Path::new(&self.options.output_file).parent()
            && !parent.as_os_str().is_empty()
            && let Err(err) = fs::create_dir_all(parent)
        {
            eprintln!(
                "Error: Failed to create output directory `{}`: {}",
                parent.display(),
                err
            );
            return false;
        }

        let mut cmd = match self.build_cc_compile_command(input_file, &target) {
            Ok(cmd) => cmd,
            Err(message) => {
                eprintln!("Error: {message}");
                return false;
            }
        };
        self.maybe_print_cc_command(&cmd);
        match cmd.status() {
            Ok(status) if status.success() => {
                if self.options.report_progress {
                    println!(
                        "Successfully compiled C-family source to `{}`",
                        self.options.output_file
                    );
                }
                true
            }
            Ok(status) => {
                eprintln!("Error: C compiler failed with exit code {}", status);
                false
            }
            Err(err) => {
                eprintln!(
                    "Error: Failed to invoke C compiler (`{}`). Make sure the Kern SDK clang or a C compiler is available. ({})",
                    cmd.get_program().to_string_lossy(),
                    err
                );
                false
            }
        }
    }

    fn build_cc_compile_command(
        &self,
        input_file: &str,
        target: &LinkTarget,
    ) -> Result<Command, String> {
        let cc_compiler = self.resolve_linker_driver(target.is_windows)?;
        let mut cmd = Command::new(&cc_compiler);
        self.apply_controlled_toolchain_runtime_env(&mut cmd, target);
        cmd.arg("-c")
            .arg(input_file)
            .arg("-o")
            .arg(&self.options.output_file);

        if cc_compiler.contains("clang") {
            cmd.arg(format!("--target={}", target.triple));
        }

        match self.options.opt_level {
            OptLevel::O0 => cmd.arg("-O0"),
            OptLevel::O1 => cmd.arg("-O1"),
            OptLevel::O2 => cmd.arg("-O2"),
            OptLevel::O3 => cmd.arg("-O3"),
        };
        if self.options.debug_info {
            cmd.arg("-g");
        }
        cmd.args(&self.options.cc_args);
        Ok(cmd)
    }

    fn build_link_command(
        &self,
        extra_inputs: &[String],
        target: &LinkTarget,
    ) -> Result<Command, String> {
        let cc_compiler = self.resolve_linker_driver(target.is_windows)?;
        let mut cmd = Command::new(&cc_compiler);
        self.apply_controlled_toolchain_driver_options(&mut cmd, target, &cc_compiler);
        self.apply_controlled_toolchain_runtime_env(&mut cmd, target);
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

        self.apply_thin_lto_cache_options(&mut cmd, target);
        self.apply_dead_strip_options(&mut cmd, target.is_windows, target.is_darwin);

        Ok(cmd)
    }

    fn apply_controlled_toolchain_driver_options(
        &self,
        cmd: &mut Command,
        target: &LinkTarget,
        cc_compiler: &str,
    ) {
        if target.is_windows || !cc_compiler.contains("clang") {
            return;
        }

        let Some(toolchain_root) = resolved_toolchain_root(&self.options) else {
            return;
        };
        let bin_dir = toolchain_bin_dir(&toolchain_root);
        if !bin_dir.is_dir() {
            return;
        }

        if !self
            .options
            .linker_args
            .iter()
            .any(|arg| arg.starts_with("-B"))
        {
            cmd.arg(format!("-B{}", bin_dir.display()));
        }

        if !self.requests_llvm_lto()
            || self
                .options
                .linker_args
                .iter()
                .any(|arg| arg.starts_with("-fuse-ld="))
        {
            return;
        }

        let lld_tool = if target.is_darwin {
            "ld64.lld"
        } else {
            "ld.lld"
        };
        if find_llvm_tool_in_root(&toolchain_root, lld_tool, false).is_some() {
            cmd.arg("-fuse-ld=lld");
        }
    }

    fn uses_lld_linker(&self, target: &LinkTarget) -> bool {
        if target.is_windows {
            return false;
        }

        if self
            .options
            .linker_args
            .iter()
            .any(|arg| arg == "-fuse-ld=lld" || arg == "-fuse-ld=ld64.lld")
        {
            return true;
        }

        if !self.requests_llvm_lto() {
            return false;
        }

        let Some(toolchain_root) = resolved_toolchain_root(&self.options) else {
            return false;
        };
        let lld_tool = if target.is_darwin {
            "ld64.lld"
        } else {
            "ld.lld"
        };
        find_llvm_tool_in_root(&toolchain_root, lld_tool, false).is_some()
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
    ) -> Result<Command, String> {
        let mut cmd = if target.is_windows {
            let llvm_lib = find_llvm_tool(&self.options, "llvm-lib", true)
                .ok_or_else(|| self.missing_default_llvm_tool_message("llvm-lib", true))?;
            Command::new(llvm_lib)
        } else {
            let cc_compiler = self.resolve_linker_driver(target.is_windows)?;
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
        self.apply_controlled_toolchain_runtime_env(&mut cmd, target);
        Ok(cmd)
    }

    fn apply_controlled_toolchain_runtime_env(&self, cmd: &mut Command, target: &LinkTarget) {
        if target.is_windows {
            return;
        }

        if let Some(toolchain_root) = resolved_toolchain_root(&self.options) {
            let runtime_dirs = [toolchain_root.join("lib"), toolchain_root.join("lib64")]
                .into_iter()
                .filter(|path| path.is_dir())
                .collect::<Vec<_>>();

            if !runtime_dirs.is_empty() {
                let var_name = if target.is_darwin {
                    "DYLD_LIBRARY_PATH"
                } else {
                    "LD_LIBRARY_PATH"
                };

                let mut entries = runtime_dirs
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect::<Vec<_>>();
                if let Ok(existing) = env::var(var_name)
                    && !existing.is_empty()
                {
                    entries.push(existing);
                }
                cmd.env(var_name, entries.join(":"));
            }
        }

        if target.is_darwin
            && let Some(sdkroot) = resolved_macos_sdkroot()
        {
            cmd.env("SDKROOT", sdkroot);
        }
    }

    fn apply_runtime_contract(&self, cmd: &mut Command, is_windows: bool, is_darwin: bool) {
        if is_windows && !matches!(self.options.runtime_entry, RuntimeEntry::None) {
            cmd.arg("-lshell32");
        }

        match self.options.runtime_entry {
            RuntimeEntry::None => {
                // `runtime_entry = none` means the host linker must not inject a
                // startup contract on our behalf. Keep libc linkage orthogonal:
                // `runtime_libc = yes` drops only startup files, while
                // `runtime_libc = no` drops the default runtime libraries too.
                if is_windows {
                    cmd.arg("-Wl,/subsystem:console");
                    if runtime_links_libc(&self.options) {
                        cmd.arg("-nostartfiles");
                    } else {
                        cmd.arg("-Wno-override-module");
                        cmd.arg("-nostdlib");
                    }
                } else if is_darwin {
                    if runtime_links_libc(&self.options) {
                        cmd.arg("-nostartfiles");
                    } else {
                        cmd.arg("-nostdlib");
                    }
                } else {
                    cmd.arg("-no-pie");
                    if runtime_links_libc(&self.options) {
                        cmd.arg("-nostartfiles");
                    } else {
                        cmd.arg("-nostdlib");
                    }
                }
                if let Some(entry_symbol) = &self.options.entry_symbol {
                    if is_windows {
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

    fn apply_thin_lto_cache_options(&self, cmd: &mut Command, target: &LinkTarget) {
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

        let uses_lld = self.uses_lld_linker(target);

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

    fn maybe_print_cc_command(&self, cmd: &Command) {
        if self.options.print_link_command {
            println!("CC command: {}", self.format_command(cmd));
        }
    }

    fn maybe_print_lto_toolchain_hint(&self, target: &LinkTarget, cmd: &Command) {
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
        let toolchain_root = resolved_toolchain_root(&self.options)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unset>".to_string());
        eprintln!(
            "Note: LLVM LTO links require the final linker toolchain to match the LLVM version used for codegen."
        );
        eprintln!(
            "      Configure the build/test environment to use a matching Clang + LTO linker toolchain."
        );
        eprintln!("      Configured link driver: {}", self.options.linker_cmd);
        if let Ok(cc_env) = env::var("CC") {
            eprintln!("      CC environment: {}", cc_env);
        }
        eprintln!("      Resolved toolchain root: {}", toolchain_root);
        eprintln!("      Resolved link command: {}", self.format_command(cmd));
        eprintln!("      Current runtime LLVM_SYS prefix: {}", llvm_prefix);
        if env::var_os("KERN_DEBUG_LTO_LINK").is_some() {
            let path = env::var("PATH").unwrap_or_else(|_| "<unset>".to_string());
            eprintln!("      PATH: {}", path);
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

    fn requests_llvm_lto(&self) -> bool {
        self.options
            .linker_args
            .iter()
            .any(|arg| arg.starts_with("-flto"))
    }

    fn should_materialize_thin_lto_link_inputs(&self) -> bool {
        self.options
            .linker_args
            .iter()
            .any(|arg| arg == "-flto=thin")
            && self
                .options
                .linker_inputs
                .iter()
                .any(|input| llvm_bitcode_file(input))
    }

    fn materialize_thin_lto_link_inputs(
        &self,
        _target: &LinkTarget,
    ) -> Option<(Vec<String>, TempDirGuard)> {
        let bitcode_positions = self
            .options
            .linker_inputs
            .iter()
            .enumerate()
            .filter_map(|(index, input)| llvm_bitcode_file(input).then_some((index, input)))
            .collect::<Vec<_>>();
        if bitcode_positions.is_empty() {
            return None;
        }

        let thin_lto_output_dir = format!("{}.tmp.thinlto.d", self.options.output_file);
        if !prepare_clean_output_dir(
            std::path::Path::new(&thin_lto_output_dir),
            "ThinLTO object directory",
        ) {
            return None;
        }
        let thin_lto_cache_dir = self.make_thin_lto_cache_dir_path();
        if !ensure_output_dir(
            std::path::Path::new(&thin_lto_cache_dir),
            "ThinLTO cache directory",
        ) {
            return None;
        }
        let thin_lto_output_guard = TempDirGuard {
            path: thin_lto_output_dir.clone(),
        };

        let thin_modules = bitcode_positions
            .iter()
            .map(|(_, input)| {
                fs::read(input).map(|bitcode| ThinLtoModule {
                    identifier: (*input).clone(),
                    bitcode,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                eprintln!("Error: failed to read ThinLTO linker input: {err}");
            })
            .ok()?;

        let object_outputs = run_thin_lto(
            &thin_modules,
            &ThinLtoOptions {
                generated_objects_dir: Some(PathBuf::from(&thin_lto_output_dir)),
                cache_dir: Some(PathBuf::from(&thin_lto_cache_dir)),
            },
        )
        .map_err(|err| {
            eprintln!("Error: LLVM ThinLTO failed during link-only prelinking: {err}");
        })
        .ok()?;

        let bitcode_module_indices = bitcode_positions
            .iter()
            .enumerate()
            .map(|(original_index, (_, input))| (input.as_str(), original_index))
            .collect::<std::collections::HashMap<_, _>>();
        let mut generated_objects = object_outputs
            .into_iter()
            .enumerate()
            .map(|(index, object)| match object {
                ThinLtoObject {
                    identifier,
                    kind: ThinLtoObjectKind::File(path),
                } => {
                    let Some(&original_index) = bitcode_module_indices.get(identifier.as_str())
                    else {
                        return Err(format!(
                            "ThinLTO returned object output for unknown module `{identifier}`"
                        ));
                    };
                    Ok((original_index, path))
                }
                ThinLtoObject {
                    kind: ThinLtoObjectKind::Buffer(_),
                    ..
                } => Err(format!(
                    "ThinLTO returned an unexpected in-memory object for output #{index}"
                )),
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| eprintln!("Error: {err}"))
            .ok()?;
        if generated_objects.is_empty() {
            eprintln!(
                "Error: ThinLTO did not materialize any object files during link-only prelinking."
            );
            return None;
        }
        generated_objects.sort_by_key(|(original_index, _)| *original_index);

        let generated_object_paths = generated_objects
            .into_iter()
            .map(|(_, object_path)| object_path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let mut linker_inputs = Vec::new();
        let mut inserted_generated_objects = false;
        for input in &self.options.linker_inputs {
            if llvm_bitcode_file(input) {
                if !inserted_generated_objects {
                    linker_inputs.extend(generated_object_paths.iter().cloned());
                    inserted_generated_objects = true;
                }
                continue;
            }
            linker_inputs.push(input.clone());
        }

        Some((linker_inputs, thin_lto_output_guard))
    }

    fn linker_args_without_linker_driven_thin_lto(&self) -> Vec<String> {
        self.options
            .linker_args
            .iter()
            .filter(|arg| {
                !arg.starts_with("-flto")
                    && !arg.contains("thinlto-cache-dir")
                    && !arg.contains("cache_path_lto")
                    && !arg.contains("plugin-opt,cache-dir=")
            })
            .cloned()
            .collect()
    }
}

fn llvm_bitcode_file(path: &str) -> bool {
    file_has_llvm_bitcode_magic(std::path::Path::new(path))
}

fn ensure_output_dir(path: &std::path::Path, label: &str) -> bool {
    if path.is_file() && fs::remove_file(path).is_err() {
        eprintln!(
            "Error: Failed to remove stale {} `{}`.",
            label,
            path.display()
        );
        return false;
    }
    if let Err(err) = fs::create_dir_all(path) {
        eprintln!(
            "Error: Failed to create {} `{}`: {}",
            label,
            path.display(),
            err
        );
        return false;
    }
    true
}

fn prepare_clean_output_dir(path: &std::path::Path, label: &str) -> bool {
    if path.is_file() && fs::remove_file(path).is_err() {
        eprintln!(
            "Error: Failed to remove stale {} `{}`.",
            label,
            path.display()
        );
        return false;
    }
    if path.is_dir() && fs::remove_dir_all(path).is_err() {
        eprintln!(
            "Error: Failed to remove stale {} `{}`.",
            label,
            path.display()
        );
        return false;
    }
    ensure_output_dir(path, label)
}

fn llvm_prefix_dir() -> Option<PathBuf> {
    env::vars().find_map(|(key, value)| {
        (key.starts_with("LLVM_SYS_") && key.ends_with("_PREFIX")).then(|| PathBuf::from(value))
    })
}

fn resolved_toolchain_root(options: &kernc_utils::config::CompileOptions) -> Option<PathBuf> {
    controlled_toolchain_root(options).or_else(llvm_prefix_dir)
}

fn controlled_toolchain_root(options: &kernc_utils::config::CompileOptions) -> Option<PathBuf> {
    if let Some(configured) = &options.toolchain_root {
        let root = PathBuf::from(configured);
        return root.is_dir().then_some(root);
    }
    sdk_relative_toolchain_root().or_else(default_install_toolchain_root)
}

fn toolchain_bin_dir(root: &std::path::Path) -> PathBuf {
    let direct_bin = root.join("bin");
    if direct_bin.is_dir() {
        return direct_bin;
    }
    root.to_path_buf()
}

fn sdk_relative_toolchain_root() -> Option<PathBuf> {
    let exe_path = env::current_exe().ok()?;
    for ancestor in exe_path.ancestors() {
        if let Some(toolchain_root) = sdk_toolchain_root_from_sdk_root(ancestor) {
            return Some(toolchain_root);
        }
    }
    None
}

fn default_install_toolchain_root() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    default_install_toolchain_root_from_home(&home)
}

fn default_install_toolchain_root_from_home(home: &std::path::Path) -> Option<PathBuf> {
    sdk_toolchain_root_from_sdk_root(&home.join(".kern"))
}

fn sdk_toolchain_root_from_sdk_root(sdk_root: &std::path::Path) -> Option<PathBuf> {
    let manifest = sdk_root.join("manifest").join("sdk.json");
    let toolchain_root = sdk_root.join("toolchain").join("host");
    (manifest.is_file() && toolchain_root.is_dir()).then_some(toolchain_root)
}

fn find_llvm_tool(
    options: &kernc_utils::config::CompileOptions,
    tool: &str,
    is_windows: bool,
) -> Option<String> {
    let root = resolved_toolchain_root(options)?;
    find_llvm_tool_in_root(&root, tool, is_windows)
}

fn find_llvm_tool_in_root(root: &std::path::Path, tool: &str, is_windows: bool) -> Option<String> {
    let direct_root = root.join(if is_windows {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    });
    if direct_root.is_file() {
        return Some(direct_root.to_string_lossy().to_string());
    }

    let direct_bin = root.join("bin").join(if is_windows {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    });
    if direct_bin.is_file() {
        return Some(direct_bin.to_string_lossy().to_string());
    }

    find_llvm_tool_in_prefix(root, tool, is_windows)
}

fn find_host_c_driver(is_windows: bool) -> Option<String> {
    let names: &[&str] = if is_windows {
        &["clang.exe", "cc.exe", "gcc.exe"]
    } else {
        &["clang", "cc", "gcc"]
    };
    names.iter().find_map(|name| find_executable_in_path(name))
}

fn find_executable_in_path(name: &str) -> Option<String> {
    let path = env::var_os("PATH")?;
    find_executable_in_search_path(name, &path)
}

fn find_executable_in_search_path(name: &str, path: &std::ffi::OsStr) -> Option<String> {
    for dir in env::split_paths(path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn find_llvm_tool_in_prefix(
    prefix: &std::path::Path,
    tool: &str,
    is_windows: bool,
) -> Option<String> {
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

fn resolved_macos_sdkroot() -> Option<String> {
    if let Ok(existing) = env::var("SDKROOT")
        && !existing.trim().is_empty()
    {
        return Some(existing);
    }

    let output = Command::new("xcrun").arg("--show-sdk-path").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let sdkroot = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sdkroot.is_empty()).then_some(sdkroot)
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

    fn command_env(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs().find_map(|(name, value)| {
            if name.to_string_lossy() != key {
                return None;
            }
            value.map(|value| value.to_string_lossy().to_string())
        })
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
            linker_cmd_explicit: true,
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();

        let cmd = driver.build_link_command(&[], &target).unwrap();
        let args = command_args(&cmd);
        let cache_dir = format!("{}.thinlto-cache.d", output.to_string_lossy());

        if target.is_darwin {
            assert!(
                args.iter()
                    .any(|arg| arg == &format!("-Wl,-cache_path_lto,{}", cache_dir))
            );
        } else if !target.is_windows {
            if find_llvm_tool(&driver.options, "ld.lld", false).is_some() {
                assert!(
                    args.iter()
                        .any(|arg| arg == &format!("-Wl,--thinlto-cache-dir={}", cache_dir))
                );
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
            linker_cmd_explicit: true,
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();

        let cmd = driver.build_link_command(&[], &target).unwrap();
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

    #[test]
    fn controlled_toolchain_roots_add_bin_search_and_lld_for_thin_lto() {
        let root = std::env::temp_dir().join(format!(
            "kern_link_controlled_toolchain_{}_{}",
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

        let output = root.join("main.out");
        let driver = CompilerDriver::new(CompileOptions {
            output_file: output.to_string_lossy().to_string(),
            linker_args: vec!["-flto=thin".to_string()],
            toolchain_root: Some(root.to_string_lossy().to_string()),
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();

        if target.is_windows || target.is_darwin {
            let _ = fs::remove_dir_all(&root);
            return;
        }

        let cmd = driver.build_link_command(&[], &target).unwrap();
        let args = command_args(&cmd);

        assert_eq!(
            cmd.get_program().to_string_lossy(),
            bin_dir.join("clang").to_string_lossy()
        );
        assert!(
            args.iter()
                .any(|arg| arg == &format!("-B{}", bin_dir.display()))
        );
        assert!(args.iter().any(|arg| arg == "-fuse-ld=lld"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cc_compile_uses_controlled_toolchain_clang_by_default() {
        let root = std::env::temp_dir().join(format!(
            "kern_cc_controlled_toolchain_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("clang"), "").unwrap();

        let driver = CompilerDriver::new(CompileOptions {
            input_file: Some(root.join("demo.c").to_string_lossy().to_string()),
            output_file: root.join("demo.o").to_string_lossy().to_string(),
            driver_mode: kernc_utils::config::DriverMode::CcCompile,
            toolchain_root: Some(root.to_string_lossy().to_string()),
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();
        let cmd = driver
            .build_cc_compile_command(driver.options.input_file.as_deref().unwrap(), &target)
            .unwrap();

        assert_eq!(
            cmd.get_program().to_string_lossy(),
            bin_dir.join("clang").to_string_lossy()
        );
        assert!(
            command_args(&cmd)
                .iter()
                .any(|arg| arg == &format!("--target={}", target.triple))
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn toolchain_lookup_accepts_default_sdk_install_under_home() {
        let home = std::env::temp_dir().join(format!(
            "kern_default_sdk_home_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sdk_root = home.join(".kern");
        let manifest_dir = sdk_root.join("manifest");
        let toolchain_root = sdk_root.join("toolchain").join("host");
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::create_dir_all(toolchain_root.join("bin")).unwrap();
        fs::write(manifest_dir.join("sdk.json"), "{}").unwrap();

        assert_eq!(
            default_install_toolchain_root_from_home(&home).as_deref(),
            Some(toolchain_root.as_path())
        );

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn host_c_driver_lookup_prefers_clang_then_cc_then_gcc_from_path() {
        let root = std::env::temp_dir().join(format!(
            "kern_host_cc_lookup_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("gcc"), "").unwrap();
        fs::write(second.join("cc"), "").unwrap();
        fs::write(second.join("clang"), "").unwrap();
        let path = std::env::join_paths([first.as_path(), second.as_path()]).unwrap();

        assert_eq!(
            find_executable_in_search_path("clang", &path).as_deref(),
            Some(second.join("clang").to_string_lossy().as_ref())
        );
        assert_eq!(
            find_executable_in_search_path("cc", &path).as_deref(),
            Some(second.join("cc").to_string_lossy().as_ref())
        );
        assert_eq!(
            find_executable_in_search_path("gcc", &path).as_deref(),
            Some(first.join("gcc").to_string_lossy().as_ref())
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn default_cc_compile_rejects_missing_sdk_clang() {
        let root = std::env::temp_dir().join(format!(
            "kern_cc_missing_sdk_clang_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();

        let driver = CompilerDriver::new(CompileOptions {
            input_file: Some(root.join("demo.c").to_string_lossy().to_string()),
            output_file: root.join("demo.o").to_string_lossy().to_string(),
            driver_mode: kernc_utils::config::DriverMode::CcCompile,
            toolchain_root: Some(root.to_string_lossy().to_string()),
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();
        let err = driver
            .build_cc_compile_command(driver.options.input_file.as_deref().unwrap(), &target)
            .unwrap_err();

        assert!(err.contains("Kern SDK clang was not found"));
        assert!(err.contains("does not fall back to host `cc`"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn explicit_cc_driver_bypasses_default_sdk_clang_requirement() {
        let root = std::env::temp_dir().join(format!(
            "kern_cc_explicit_driver_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();

        let driver = CompilerDriver::new(CompileOptions {
            input_file: Some(root.join("demo.c").to_string_lossy().to_string()),
            output_file: root.join("demo.o").to_string_lossy().to_string(),
            driver_mode: kernc_utils::config::DriverMode::CcCompile,
            toolchain_root: Some(root.to_string_lossy().to_string()),
            linker_cmd: "cc".to_string(),
            linker_cmd_explicit: true,
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();
        let cmd = driver
            .build_cc_compile_command(driver.options.input_file.as_deref().unwrap(), &target)
            .unwrap();

        assert_eq!(cmd.get_program().to_string_lossy(), "cc");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn controlled_toolchain_roots_export_runtime_library_search_path() {
        let root = std::env::temp_dir().join(format!(
            "kern_link_controlled_runtime_env_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin_dir = root.join("bin");
        let lib_dir = root.join("lib");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();
        fs::write(bin_dir.join("clang"), "").unwrap();
        fs::write(bin_dir.join("ld.lld"), "").unwrap();

        let output = root.join("main.out");
        let driver = CompilerDriver::new(CompileOptions {
            output_file: output.to_string_lossy().to_string(),
            toolchain_root: Some(root.to_string_lossy().to_string()),
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();

        if target.is_windows {
            let _ = fs::remove_dir_all(&root);
            return;
        }

        let cmd = driver.build_link_command(&[], &target).unwrap();
        let env_key = if target.is_darwin {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let value = command_env(&cmd, env_key).expect("expected runtime library search path");
        assert!(
            value.starts_with(&format!("{}:", lib_dir.to_string_lossy()))
                || value == lib_dir.to_string_lossy()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn controlled_toolchain_roots_export_macos_sdkroot() {
        let root = std::env::temp_dir().join(format!(
            "kern_link_controlled_sdkroot_env_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin_dir = root.join("bin");
        let lib_dir = root.join("lib");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();
        fs::write(bin_dir.join("clang"), "").unwrap();
        fs::write(bin_dir.join("ld64.lld"), "").unwrap();

        let output = root.join("main.out");
        let driver = CompilerDriver::new(CompileOptions {
            output_file: output.to_string_lossy().to_string(),
            target: kernc_utils::config::TargetMachine::new("x86_64-apple-darwin").unwrap(),
            toolchain_root: Some(root.to_string_lossy().to_string()),
            ..CompileOptions::default()
        });
        let target = driver.normalized_target();

        if !target.is_darwin {
            let _ = fs::remove_dir_all(&root);
            return;
        }

        let Some(expected) = resolved_macos_sdkroot() else {
            let _ = fs::remove_dir_all(&root);
            return;
        };

        let cmd = driver.build_link_command(&[], &target).unwrap();
        let value = command_env(&cmd, "SDKROOT").expect("expected SDKROOT for Darwin link env");
        assert_eq!(value, expected);

        let _ = fs::remove_dir_all(&root);
    }
}
