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
pub enum LinkProfile {
    Kern,
    Freestanding,
    Hosted,
    None,
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
    /// User-defined compile-time configuration injected from the CLI, for example `-D feature=true`.
    pub custom_defines: HashMap<String, String>,
    /// `craft` project analysis feature selection used by tools such as `kern-lsp`.
    pub craft_features: Vec<String>,
    pub craft_default_features: bool,
    // Module alias mapping table.
    pub module_aliases: HashMap<String, String>,
    // Interface alias mapping table rooted at the `kmeta` directory.
    pub module_interface_aliases: HashMap<String, String>,
    pub asm_dialect: AsmDialect,
    pub link_profile: LinkProfile,
    pub linker_cmd: String,
    pub linker_inputs: Vec<String>,
    pub linker_search_paths: Vec<String>,
    pub linker_libraries: Vec<String>,
    pub linker_args: Vec<String>,
    pub entry_symbol: Option<String>,
    pub print_link_command: bool,
    pub report_progress: bool,
    pub use_std: bool,
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
            link_profile: LinkProfile::Kern,
            linker_cmd: "cc".to_string(),
            linker_inputs: Vec::new(),
            linker_search_paths: Vec::new(),
            linker_libraries: Vec::new(),
            linker_args: Vec::new(),
            entry_symbol: None,
            print_link_command: false,
            report_progress: true,
            use_std: false,
        }
    }
}

pub fn resolve_std_path() -> PathBuf {
    if let Ok(custom_std) = env::var("KERN_STD_PATH") {
        return PathBuf::from(custom_std);
    }

    if let Ok(exe_path) = env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        for ancestor in exe_dir.ancestors() {
            let candidate = ancestor.join("library/std");
            if candidate.join("init.rn").is_file() {
                return candidate;
            }
        }
        for ancestor in exe_dir.ancestors() {
            let candidate = ancestor.join("lib/kern/std");
            if candidate.join("init.rn").is_file() {
                return candidate;
            }
        }
    }

    PathBuf::from("library/std")
}

pub fn maybe_inject_std_alias(options: &mut CompileOptions) {
    if !options.use_std || options.module_aliases.contains_key("std") {
        return;
    }

    let std_path = resolve_std_path();
    options
        .module_aliases
        .insert("std".to_string(), std_path.to_string_lossy().to_string());
}

pub fn inject_driver_condition_defines(options: &mut CompileOptions) {
    let link_profile = match options.link_profile {
        LinkProfile::Kern => "kern",
        LinkProfile::Freestanding => "freestanding",
        LinkProfile::Hosted => "hosted",
        LinkProfile::None => "none",
    };

    let hosted = matches!(options.link_profile, LinkProfile::Hosted);
    let kern_rt = options.use_std && !hosted;

    options
        .custom_defines
        .insert("link_profile".to_string(), link_profile.to_string());
    options
        .custom_defines
        .insert("hosted".to_string(), hosted.to_string());
    options
        .custom_defines
        .insert("libc".to_string(), hosted.to_string());
    options
        .custom_defines
        .insert("kern_rt".to_string(), kern_rt.to_string());
}
