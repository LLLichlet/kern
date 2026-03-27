use std::collections::HashMap;
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
    /// 从字符串解析目标架构 (例如 "x86_64-unknown-linux-gnu")
    pub fn new(triple_str: &str) -> Result<Self, String> {
        let triple = Triple::from_str(triple_str).map_err(|e| e.to_string())?;

        let pointer_size = match triple.pointer_width() {
            Ok(PointerWidth::U16) => 2,
            Ok(PointerWidth::U32) => 4,
            Ok(PointerWidth::U64) => 8,
            Err(_) => 8, // 默认 fallback 到 64-bit
        };

        Ok(Self {
            triple,
            pointer_size,
        })
    }
}

impl Default for TargetMachine {
    fn default() -> Self {
        // 默认使用当前宿主机的架构
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
    pub target: TargetMachine,
    pub opt_level: OptLevel,
    pub driver_mode: DriverMode,
    /// 允许通过 CLI 传入自定义环境变量，例如 kernc -D my_feature=true
    pub custom_defines: HashMap<String, String>,
    // 模块别名映射表
    pub module_aliases: HashMap<String, String>,
    pub asm_dialect: AsmDialect,
    pub link_profile: LinkProfile,
    pub linker_cmd: String,
    pub linker_inputs: Vec<String>,
    pub linker_search_paths: Vec<String>,
    pub linker_libraries: Vec<String>,
    pub linker_args: Vec<String>,
    pub entry_symbol: Option<String>,
    pub print_link_command: bool,
    pub use_std: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            input_file: None,
            output_file: "a.out".to_string(),
            target: TargetMachine::default(),
            opt_level: OptLevel::O0,
            driver_mode: DriverMode::CompileAndLink,
            custom_defines: HashMap::new(),
            module_aliases: HashMap::new(),
            asm_dialect: AsmDialect::default(),
            link_profile: LinkProfile::Kern,
            linker_cmd: "cc".to_string(),
            linker_inputs: Vec::new(),
            linker_search_paths: Vec::new(),
            linker_libraries: Vec::new(),
            linker_args: Vec::new(),
            entry_symbol: None,
            print_link_command: false,
            use_std: false,
        }
    }
}
