use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use target_lexicon::{PointerWidth, Triple};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    O0,
    O1,
    O2,
    O3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverMode {
    CompileAndLink,
    CompileOnly,
    LinkOnly,
    EmitLlvmIr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlvmIrStage {
    #[default]
    Raw,
    Verified,
    Optimized,
}

impl LlvmIrStage {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "raw" => Ok(Self::Raw),
            "verified" => Ok(Self::Verified),
            "optimized" => Ok(Self::Optimized),
            _ => Err(format!(
                "invalid LLVM IR stage `{value}`; expected one of: raw, verified, optimized"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Verified => "verified",
            Self::Optimized => "optimized",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LtoMode {
    #[default]
    None,
    Full,
    Thin,
}

impl LtoMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "full" => Ok(Self::Full),
            "thin" => Ok(Self::Thin),
            _ => Err(format!(
                "invalid LTO mode `{value}`; expected one of: none, full, thin"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Full => "full",
            Self::Thin => "thin",
        }
    }
}

impl DriverMode {
    pub fn needs_source_input(self) -> bool {
        !matches!(self, DriverMode::LinkOnly)
    }

    pub fn performs_codegen(self) -> bool {
        !matches!(self, DriverMode::LinkOnly)
    }

    pub fn performs_link(self) -> bool {
        matches!(self, DriverMode::CompileAndLink | DriverMode::LinkOnly)
    }

    pub fn emits_linker_input(self) -> bool {
        matches!(self, DriverMode::CompileOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeEntry {
    None,
    Rt,
    Crt,
}

impl RuntimeEntry {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "rt" => Ok(Self::Rt),
            "crt" => Ok(Self::Crt),
            _ => Err(format!(
                "invalid runtime entry `{value}`; expected one of: none, rt, crt"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Rt => "rt",
            Self::Crt => "crt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryBundle {
    None,
    Base,
    Std,
}

impl LibraryBundle {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "base" => Ok(Self::Base),
            "std" => Ok(Self::Std),
            _ => Err(format!(
                "invalid library bundle `{value}`; expected one of: none, base, std"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Base => "base",
            Self::Std => "std",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfficialLibrary {
    Base,
    Rt,
    Sys,
    Std,
}

impl OfficialLibrary {
    const fn alias(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Rt => "rt",
            Self::Sys => "sys",
            Self::Std => "std",
        }
    }

    const fn env_var(self) -> &'static str {
        match self {
            Self::Base => "KERN_BASE_PATH",
            Self::Rt => "KERN_RT_PATH",
            Self::Sys => "KERN_SYS_PATH",
            Self::Std => "KERN_STD_PATH",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TargetMachine {
    pub triple: Triple,
    pub pointer_size: u64,
}

impl TargetMachine {
    /// Parse a target machine description such as `x86_64-unknown-linux-gnu`.
    pub fn new(triple_str: &str) -> Result<Self, String> {
        let triple = Triple::from_str(triple_str).map_err(|e| e.to_string())?;

        let pointer_size = match triple.pointer_width() {
            Ok(PointerWidth::U16) => 2,
            Ok(PointerWidth::U32) => 4,
            Ok(PointerWidth::U64) => 8,
            Err(_) => 8, // Default to 64-bit on unknown pointer widths.
        };

        Ok(Self {
            triple,
            pointer_size,
        })
    }

    pub fn max_lock_free_atomic_bits(&self) -> u64 {
        (self.pointer_size * 16).min(128)
    }
}

impl Default for TargetMachine {
    fn default() -> Self {
        // Default to the host architecture.
        let triple = Triple::host();
        let pointer_size = match triple.pointer_width() {
            Ok(PointerWidth::U16) => 2,
            Ok(PointerWidth::U32) => 4,
            Ok(PointerWidth::U64) => 8,
            Err(_) => 8,
        };
        Self {
            triple,
            pointer_size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AsmDialect {
    #[default]
    Intel,
    Att,
}

#[derive(Debug, Clone)]
pub struct CompileOptions {
    pub input_file: Option<String>,
    pub output_file: String,
    pub metadata_output: Option<String>,
    pub metadata_package_name: Option<String>,
    pub metadata_package_version: Option<String>,
    pub root_module_name: Option<String>,
    pub target: TargetMachine,
    pub opt_level: OptLevel,
    pub driver_mode: DriverMode,
    /// User-defined compile-time configuration injected from the CLI, for example `--define feature=true`.
    pub custom_defines: HashMap<String, String>,
    /// `craft` project analysis feature selection used by tools such as `kern-lsp`.
    pub craft_features: Vec<String>,
    pub craft_default_features: bool,
    // Module alias mapping table.
    pub module_aliases: HashMap<String, String>,
    // Interface alias mapping table rooted at the `kmeta` directory.
    pub module_interface_aliases: HashMap<String, String>,
    pub asm_dialect: AsmDialect,
    pub linker_cmd: String,
    pub linker_inputs: Vec<String>,
    pub linker_search_paths: Vec<String>,
    pub linker_libraries: Vec<String>,
    pub linker_args: Vec<String>,
    pub entry_symbol: Option<String>,
    pub runtime_entry: RuntimeEntry,
    pub runtime_libc: bool,
    pub library_bundle: LibraryBundle,
    pub codegen_units: usize,
    pub lto_mode: LtoMode,
    pub emit_llvm_stage: LlvmIrStage,
    pub emit_multi_object_dir: bool,
    pub split_sections_for_gc: bool,
    pub dead_strip_sections: bool,
    pub print_link_command: bool,
    pub report_progress: bool,
    pub report_timings: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            input_file: None,
            output_file: "a.out".to_string(),
            metadata_output: None,
            metadata_package_name: None,
            metadata_package_version: None,
            root_module_name: None,
            target: TargetMachine::default(),
            opt_level: OptLevel::O0,
            driver_mode: DriverMode::CompileAndLink,
            custom_defines: HashMap::new(),
            craft_features: Vec::new(),
            craft_default_features: true,
            module_aliases: HashMap::new(),
            module_interface_aliases: HashMap::new(),
            asm_dialect: AsmDialect::default(),
            linker_cmd: "cc".to_string(),
            linker_inputs: Vec::new(),
            linker_search_paths: Vec::new(),
            linker_libraries: Vec::new(),
            linker_args: Vec::new(),
            entry_symbol: None,
            runtime_entry: RuntimeEntry::None,
            runtime_libc: false,
            library_bundle: LibraryBundle::None,
            codegen_units: 1,
            lto_mode: LtoMode::default(),
            emit_llvm_stage: LlvmIrStage::default(),
            emit_multi_object_dir: false,
            split_sections_for_gc: false,
            dead_strip_sections: false,
            print_link_command: false,
            report_progress: true,
            report_timings: false,
        }
    }
}

fn resolve_official_library_path(library: OfficialLibrary) -> PathBuf {
    if let Ok(custom_path) = env::var(library.env_var()) {
        return PathBuf::from(custom_path);
    }

    if let Ok(exe_path) = env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        for ancestor in exe_dir.ancestors() {
            let candidate = ancestor.join("library").join(library.alias());
            if candidate.join("init.rn").is_file() {
                return candidate;
            }
        }
        for ancestor in exe_dir.ancestors() {
            let candidate = ancestor.join("lib/kern").join(library.alias());
            if candidate.join("init.rn").is_file() {
                return candidate;
            }
        }
    }

    PathBuf::from("library").join(library.alias())
}

pub fn resolve_std_path() -> PathBuf {
    resolve_official_library_path(OfficialLibrary::Std)
}

pub fn resolve_base_path() -> PathBuf {
    resolve_official_library_path(OfficialLibrary::Base)
}

pub fn resolve_rt_path() -> PathBuf {
    resolve_official_library_path(OfficialLibrary::Rt)
}

pub fn resolve_sys_path() -> PathBuf {
    resolve_official_library_path(OfficialLibrary::Sys)
}

fn ensure_official_library_alias(options: &mut CompileOptions, library: OfficialLibrary) {
    if options.module_aliases.contains_key(library.alias()) {
        return;
    }

    let path = match library {
        OfficialLibrary::Base => resolve_base_path(),
        OfficialLibrary::Rt => resolve_rt_path(),
        OfficialLibrary::Sys => resolve_sys_path(),
        OfficialLibrary::Std => resolve_std_path(),
    };
    options.module_aliases.insert(
        library.alias().to_string(),
        path.to_string_lossy().to_string(),
    );
}

pub fn maybe_add_base_alias(options: &mut CompileOptions) {
    let wants_base = matches!(
        options.library_bundle,
        LibraryBundle::Base | LibraryBundle::Std
    );
    if !wants_base || options.module_aliases.contains_key("base") {
        return;
    }

    ensure_official_library_alias(options, OfficialLibrary::Base);
}

pub fn maybe_add_rt_alias(options: &mut CompileOptions) {
    if matches!(options.runtime_entry, RuntimeEntry::None)
        || options.module_aliases.contains_key("rt")
    {
        return;
    }

    ensure_official_library_alias(options, OfficialLibrary::Rt);
}

pub fn maybe_add_sys_alias(options: &mut CompileOptions) {
    let wants_sys = matches!(options.library_bundle, LibraryBundle::Std);
    if !wants_sys || options.module_aliases.contains_key("sys") {
        return;
    }

    ensure_official_library_alias(options, OfficialLibrary::Sys);
}

pub fn maybe_add_std_alias(options: &mut CompileOptions) {
    if !matches!(options.library_bundle, LibraryBundle::Std)
        || options.module_aliases.contains_key("std")
    {
        return;
    }

    ensure_official_library_alias(options, OfficialLibrary::Std);
}

pub fn apply_configured_library_aliases(options: &mut CompileOptions) {
    maybe_add_base_alias(options);
    maybe_add_rt_alias(options);
    maybe_add_sys_alias(options);
    maybe_add_std_alias(options);
}

pub fn runtime_links_libc(options: &CompileOptions) -> bool {
    options.runtime_libc
}

pub fn runtime_uses_crt_startup(options: &CompileOptions) -> bool {
    matches!(options.runtime_entry, RuntimeEntry::Crt)
}

pub fn validate_runtime_options(options: &CompileOptions) -> Result<(), String> {
    if matches!(options.runtime_entry, RuntimeEntry::Crt) && !runtime_links_libc(options) {
        return Err(
            "invalid runtime configuration: `runtime_entry = crt` requires libc linkage"
                .to_string(),
        );
    }

    Ok(())
}

pub fn validate_compile_options(options: &CompileOptions) -> Result<(), String> {
    validate_runtime_options(options)?;

    if matches!(options.driver_mode, DriverMode::LinkOnly)
        && !matches!(options.lto_mode, LtoMode::None)
    {
        return Err(
            "invalid compile configuration: `--lto` requires frontend/codegen; `--link-only` cannot perform LTO"
                .to_string(),
        );
    }

    if matches!(options.driver_mode, DriverMode::EmitLlvmIr)
        && options.codegen_units > 1
        && !matches!(options.lto_mode, LtoMode::Full)
    {
        return Err(
            "invalid compile configuration: `--emit-llvm` with multiple codegen units requires `--lto full`"
                .to_string(),
        );
    }

    if options.emit_multi_object_dir && matches!(options.lto_mode, LtoMode::Full) {
        return Err(
            "invalid compile configuration: preserving per-CGU object directories is incompatible with `--lto full`"
                .to_string(),
        );
    }

    Ok(())
}

pub fn inject_driver_condition_defines(options: &mut CompileOptions) {
    options.custom_defines.insert(
        "runtime_entry".to_string(),
        options.runtime_entry.as_str().to_string(),
    );
    options.custom_defines.insert(
        "library_bundle".to_string(),
        options.library_bundle.as_str().to_string(),
    );
    options
        .custom_defines
        .insert("libc".to_string(), runtime_links_libc(options).to_string());
    options.custom_defines.insert(
        "crt_startup".to_string(),
        runtime_uses_crt_startup(options).to_string(),
    );
    options
        .custom_defines
        .entry("rt_role".to_string())
        .or_insert_with(|| "default".to_string());
}
