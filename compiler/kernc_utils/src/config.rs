use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use target_lexicon::{Architecture, PointerWidth, Triple};

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
    CcCompile,
    AnalyzeOnly,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkerInputFlavor {
    #[default]
    Object,
    ThinLtoBitcode,
}

impl LinkerInputFlavor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::ThinLtoBitcode => "thinlto-bitcode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeModel {
    #[default]
    Default,
    Small,
    Kernel,
    Medium,
    Large,
}

impl CodeModel {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "default" => Ok(Self::Default),
            "small" => Ok(Self::Small),
            "kernel" => Ok(Self::Kernel),
            "medium" => Ok(Self::Medium),
            "large" => Ok(Self::Large),
            _ => Err(format!(
                "invalid code model `{value}`; expected one of: default, small, kernel, medium, large"
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Small => "small",
            Self::Kernel => "kernel",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

impl DriverMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CompileAndLink => "compile-and-link",
            Self::CompileOnly => "compile-only",
            Self::CcCompile => "cc-compile",
            Self::AnalyzeOnly => "analyze-only",
            Self::LinkOnly => "link-only",
            Self::EmitLlvmIr => "emit-llvm-ir",
        }
    }

    pub fn needs_source_input(self) -> bool {
        !matches!(self, DriverMode::LinkOnly)
    }

    pub fn performs_codegen(self) -> bool {
        !matches!(self, DriverMode::LinkOnly | DriverMode::AnalyzeOnly)
    }

    pub fn performs_link(self) -> bool {
        matches!(self, DriverMode::CompileAndLink | DriverMode::LinkOnly)
    }

    pub fn emits_linker_input(self) -> bool {
        matches!(self, DriverMode::CompileOnly | DriverMode::CcCompile)
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
    Prov,
    Std,
}

impl OfficialLibrary {
    const fn alias(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Rt => "rt",
            Self::Prov => "prov",
            Self::Std => "std",
        }
    }

    const fn env_var(self) -> &'static str {
        match self {
            Self::Base => "KERN_BASE_PATH",
            Self::Rt => "KERN_RT_PATH",
            Self::Prov => "KERN_PROV_PATH",
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
    Auto,
    Intel,
    Att,
}

impl AsmDialect {
    pub fn effective_for_target(self, target: &TargetMachine) -> Self {
        match self {
            Self::Auto => {
                if matches!(
                    target.triple.architecture,
                    Architecture::X86_64 | Architecture::X86_32(_)
                ) {
                    Self::Intel
                } else {
                    Self::Att
                }
            }
            explicit => explicit,
        }
    }
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
    pub toolchain_root: Option<String>,
    pub linker_cmd: String,
    pub linker_cmd_explicit: bool,
    pub linker_inputs: Vec<String>,
    pub linker_search_paths: Vec<String>,
    pub linker_libraries: Vec<String>,
    pub linker_args: Vec<String>,
    pub cc_args: Vec<String>,
    pub entry_symbol: Option<String>,
    pub runtime_entry: RuntimeEntry,
    pub runtime_libc: bool,
    pub library_bundle: LibraryBundle,
    pub codegen_units: usize,
    pub lto_mode: LtoMode,
    pub code_model: CodeModel,
    pub linker_input_flavor: LinkerInputFlavor,
    pub emit_llvm_stage: LlvmIrStage,
    pub emit_multi_linker_input_dir: bool,
    pub split_sections_for_gc: bool,
    pub debug_info: bool,
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
            toolchain_root: None,
            linker_cmd: "cc".to_string(),
            linker_cmd_explicit: false,
            linker_inputs: Vec::new(),
            linker_search_paths: Vec::new(),
            linker_libraries: Vec::new(),
            linker_args: Vec::new(),
            cc_args: Vec::new(),
            entry_symbol: None,
            runtime_entry: RuntimeEntry::None,
            runtime_libc: false,
            library_bundle: LibraryBundle::None,
            codegen_units: 1,
            lto_mode: LtoMode::default(),
            code_model: CodeModel::default(),
            linker_input_flavor: LinkerInputFlavor::default(),
            emit_llvm_stage: LlvmIrStage::default(),
            emit_multi_linker_input_dir: false,
            split_sections_for_gc: false,
            debug_info: false,
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

pub fn resolve_prov_path() -> PathBuf {
    resolve_official_library_path(OfficialLibrary::Prov)
}

fn ensure_official_library_alias(options: &mut CompileOptions, library: OfficialLibrary) {
    if options.module_aliases.contains_key(library.alias()) {
        return;
    }

    let path = match library {
        OfficialLibrary::Base => resolve_base_path(),
        OfficialLibrary::Rt => resolve_rt_path(),
        OfficialLibrary::Prov => resolve_prov_path(),
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

pub fn maybe_add_prov_alias(options: &mut CompileOptions) {
    let wants_prov = matches!(options.library_bundle, LibraryBundle::Std);
    if !wants_prov || options.module_aliases.contains_key("prov") {
        return;
    }

    ensure_official_library_alias(options, OfficialLibrary::Prov);
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
    maybe_add_prov_alias(options);
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

    if options.emit_multi_linker_input_dir && matches!(options.lto_mode, LtoMode::Full) {
        return Err(
            "invalid compile configuration: preserving per-CGU object directories is incompatible with `--lto full`"
                .to_string(),
        );
    }

    if matches!(
        options.linker_input_flavor,
        LinkerInputFlavor::ThinLtoBitcode
    ) {
        if !options.driver_mode.emits_linker_input() {
            return Err(
                "invalid compile configuration: `thinlto-bitcode` linker-input emission requires `--compile-only`"
                    .to_string(),
            );
        }

        if !matches!(options.lto_mode, LtoMode::Thin) {
            return Err(
                "invalid compile configuration: `thinlto-bitcode` linker-input emission requires `--lto thin`"
                    .to_string(),
            );
        }

        if options.codegen_units > 1 && !options.emit_multi_linker_input_dir {
            return Err(
                "invalid compile configuration: multi-CGU `thinlto-bitcode` emission requires preserving a per-CGU linker-input directory"
                    .to_string(),
            );
        }
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
