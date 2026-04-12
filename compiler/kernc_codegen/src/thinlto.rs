use llvm_sys::lto::{
    LTOObjectBuffer, lto_codegen_model, lto_get_error_message,
    lto_module_create_from_memory_with_path, lto_module_dispose, lto_module_get_num_symbols,
    lto_module_get_symbol_attribute, lto_module_get_symbol_name, thinlto_code_gen_t,
    thinlto_codegen_add_cross_referenced_symbol, thinlto_codegen_add_module,
    thinlto_codegen_add_must_preserve_symbol, thinlto_codegen_dispose, thinlto_codegen_process,
    thinlto_codegen_set_cpu, thinlto_codegen_set_pic_model, thinlto_create_codegen,
    thinlto_module_get_num_objects, thinlto_module_get_object,
};
use std::collections::BTreeSet;
use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::slice;

const LTO_SYMBOL_DEFINITION_MASK: u32 = 0x700;
const LTO_SYMBOL_DEFINITION_REGULAR: u32 = 0x100;
const LTO_SYMBOL_DEFINITION_TENTATIVE: u32 = 0x200;
const LTO_SYMBOL_DEFINITION_WEAK: u32 = 0x300;
const LTO_SYMBOL_DEFINITION_UNDEFINED: u32 = 0x400;
const LTO_SYMBOL_DEFINITION_WEAKUNDEF: u32 = 0x500;
const LTO_SYMBOL_SCOPE_MASK: u32 = 0x3800;
const LTO_SYMBOL_SCOPE_INTERNAL: u32 = 0x800;

#[derive(Debug, Clone)]
pub struct ThinLtoModule {
    pub identifier: String,
    pub bitcode: Vec<u8>,
}

struct ThinLtoCodegenGuard(thinlto_code_gen_t);

impl Drop for ThinLtoCodegenGuard {
    fn drop(&mut self) {
        unsafe { thinlto_codegen_dispose(self.0) };
    }
}

struct ThinLtoModuleGuard(llvm_sys::lto::lto_module_t);

impl Drop for ThinLtoModuleGuard {
    fn drop(&mut self) {
        unsafe { lto_module_dispose(self.0) };
    }
}

#[repr(C)]
struct RawLtoObjectBuffer {
    buffer: *const std::os::raw::c_char,
    size: usize,
}

pub fn run_thin_lto(modules: &[ThinLtoModule]) -> Result<Vec<Vec<u8>>, String> {
    if modules.is_empty() {
        return Ok(Vec::new());
    }

    let cg = unsafe { thinlto_create_codegen() };
    if cg.is_null() {
        return Err(last_lto_error(
            "LLVM ThinLTO failed to create a code generator".to_string(),
        ));
    }
    let cg = ThinLtoCodegenGuard(cg);

    let cpu = CString::new("generic").unwrap();
    unsafe {
        thinlto_codegen_set_cpu(cg.0, cpu.as_ptr());
    }
    if unsafe {
        thinlto_codegen_set_pic_model(cg.0, lto_codegen_model::LTO_CODEGEN_PIC_MODEL_DEFAULT)
    } != 0
    {
        return Err(last_lto_error(
            "LLVM ThinLTO failed to configure the PIC model".to_string(),
        ));
    }

    let (must_preserve, cross_referenced) = collect_symbol_policy(modules)?;
    let preserved_symbols = must_preserve
        .iter()
        .map(|name| {
            CString::new(name.as_slice()).map_err(|_| {
                format!(
                    "ThinLTO preserve-symbol name contains an interior NUL byte: {:?}",
                    name
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let cross_referenced_symbols = cross_referenced
        .iter()
        .map(|name| {
            CString::new(name.as_slice()).map_err(|_| {
                format!(
                    "ThinLTO cross-reference symbol name contains an interior NUL byte: {:?}",
                    name
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    for name in &preserved_symbols {
        unsafe {
            thinlto_codegen_add_must_preserve_symbol(
                cg.0,
                name.as_ptr() as *const _,
                name.as_bytes_with_nul()
                    .len()
                    .try_into()
                    .map_err(|_| format!("ThinLTO symbol is too long to preserve: {:?}", name))?,
            );
        }
    }
    for name in &cross_referenced_symbols {
        unsafe {
            thinlto_codegen_add_cross_referenced_symbol(
                cg.0,
                name.as_ptr() as *const _,
                name.as_bytes_with_nul().len().try_into().map_err(|_| {
                    format!(
                        "ThinLTO symbol is too long to mark cross-referenced: {:?}",
                        name
                    )
                })?,
            );
        }
    }

    let identifiers = modules
        .iter()
        .map(|module| {
            CString::new(module.identifier.as_str()).map_err(|_| {
                format!(
                    "ThinLTO module identifier contains an interior NUL byte: {:?}",
                    module.identifier
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (module, identifier) in modules.iter().zip(identifiers.iter()) {
        let length: c_int = module.bitcode.len().try_into().map_err(|_| {
            format!(
                "ThinLTO bitcode module is too large: `{}`",
                module.identifier
            )
        })?;
        unsafe {
            thinlto_codegen_add_module(
                cg.0,
                identifier.as_ptr(),
                module.bitcode.as_ptr() as *const _,
                length,
            );
        }
    }

    unsafe {
        thinlto_codegen_process(cg.0);
    }

    let object_count = unsafe { thinlto_module_get_num_objects(cg.0) as usize };
    if object_count == 0 {
        return Err(last_lto_error(
            "LLVM ThinLTO did not produce any object files".to_string(),
        ));
    }

    let mut objects = Vec::with_capacity(object_count);
    for index in 0..object_count {
        let object = unsafe { thinlto_module_get_object(cg.0, index as u32) };
        objects.push(copy_object_buffer(index, object)?);
    }
    Ok(objects)
}

fn collect_symbol_policy(
    modules: &[ThinLtoModule],
) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>), String> {
    let mut must_preserve = BTreeSet::<Vec<u8>>::new();
    let mut defined = BTreeSet::<Vec<u8>>::new();
    let mut undefined = BTreeSet::<Vec<u8>>::new();

    for module in modules {
        let identifier = CString::new(module.identifier.as_str()).map_err(|_| {
            format!(
                "ThinLTO module identifier contains an interior NUL byte: {:?}",
                module.identifier
            )
        })?;
        let lto_module = unsafe {
            lto_module_create_from_memory_with_path(
                module.bitcode.as_ptr() as *const _,
                module.bitcode.len(),
                identifier.as_ptr(),
            )
        };
        if lto_module.is_null() {
            return Err(last_lto_error(format!(
                "LLVM ThinLTO could not inspect module `{}`",
                module.identifier
            )));
        }
        let lto_module = ThinLtoModuleGuard(lto_module);
        let symbol_count = unsafe { lto_module_get_num_symbols(lto_module.0) as usize };
        for index in 0..symbol_count {
            let name_ptr = unsafe { lto_module_get_symbol_name(lto_module.0, index as u32) };
            if name_ptr.is_null() {
                continue;
            }
            let name = unsafe { CStr::from_ptr(name_ptr) }.to_bytes().to_vec();
            let attrs =
                unsafe { lto_module_get_symbol_attribute(lto_module.0, index as u32) } as u32;
            if is_definition(attrs) {
                if !is_internal_scope(attrs) {
                    must_preserve.insert(name.clone());
                    defined.insert(name);
                }
            } else if is_undefined(attrs) {
                undefined.insert(name);
            }
        }
    }

    let cross_referenced = defined
        .into_iter()
        .filter(|name| !must_preserve.contains(name) && undefined.contains(name))
        .collect::<Vec<_>>();
    Ok((must_preserve.into_iter().collect(), cross_referenced))
}

fn copy_object_buffer(index: usize, object: LTOObjectBuffer) -> Result<Vec<u8>, String> {
    let object: RawLtoObjectBuffer = unsafe { std::mem::transmute(object) };
    if object.buffer.is_null() {
        return Err(format!(
            "LLVM ThinLTO returned a null object buffer for output #{index}"
        ));
    }
    Ok(unsafe { slice::from_raw_parts(object.buffer as *const u8, object.size).to_vec() })
}

fn is_definition(attrs: u32) -> bool {
    matches!(
        attrs & LTO_SYMBOL_DEFINITION_MASK,
        LTO_SYMBOL_DEFINITION_REGULAR
            | LTO_SYMBOL_DEFINITION_TENTATIVE
            | LTO_SYMBOL_DEFINITION_WEAK
    )
}

fn is_undefined(attrs: u32) -> bool {
    matches!(
        attrs & LTO_SYMBOL_DEFINITION_MASK,
        LTO_SYMBOL_DEFINITION_UNDEFINED | LTO_SYMBOL_DEFINITION_WEAKUNDEF
    )
}

fn is_internal_scope(attrs: u32) -> bool {
    (attrs & LTO_SYMBOL_SCOPE_MASK) == LTO_SYMBOL_SCOPE_INTERNAL
}

fn last_lto_error(fallback: String) -> String {
    let message = unsafe { lto_get_error_message() };
    if message.is_null() {
        fallback
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    }
}
