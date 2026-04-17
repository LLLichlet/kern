use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ThinLtoModule {
    pub identifier: String,
    pub bitcode: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct ThinLtoOptions {
    pub generated_objects_dir: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinLtoObject {
    Buffer(Vec<u8>),
    File(PathBuf),
}

pub fn run_thin_lto(
    modules: &[ThinLtoModule],
    options: &ThinLtoOptions,
) -> Result<Vec<ThinLtoObject>, String> {
    if modules.is_empty() {
        return Ok(Vec::new());
    }

    let session = ThinLtoSession::new()?;
    session.set_cpu("generic")?;

    if let Some(generated_objects_dir) = options.generated_objects_dir.as_deref() {
        session.set_generated_objects_dir(generated_objects_dir)?;
    }
    if let Some(cache_dir) = options.cache_dir.as_deref() {
        session.set_cache_dir(cache_dir)?;
    }

    for module in modules {
        session.add_module(module)?;
    }

    session.process()?;

    let object_count = session.object_count();
    if object_count == 0 {
        return Err(session.last_error("LLVM ThinLTO did not produce any object files".to_string()));
    }

    let mut objects = Vec::with_capacity(object_count);
    for index in 0..object_count {
        objects.push(session.object(index)?);
    }
    Ok(objects)
}

struct ThinLtoSession(*mut ThinLtoSessionOpaque);

impl ThinLtoSession {
    fn new() -> Result<Self, String> {
        let raw = unsafe { kern_thinlto_session_create() };
        if raw.is_null() {
            Err("LLVM ThinLTO failed to allocate a bridge session".to_string())
        } else {
            Ok(Self(raw))
        }
    }

    fn set_cpu(&self, cpu: &str) -> Result<(), String> {
        let cpu = CString::new(cpu).unwrap();
        self.run_bool(
            unsafe { kern_thinlto_session_set_cpu(self.0, cpu.as_ptr()) },
            "LLVM ThinLTO failed to configure the target CPU",
        )
    }

    fn set_generated_objects_dir(&self, dir: &Path) -> Result<(), String> {
        let dir = c_string_path(dir, "ThinLTO generated-objects directory")?;
        self.run_bool(
            unsafe { kern_thinlto_session_set_generated_objects_dir(self.0, dir.as_ptr()) },
            "LLVM ThinLTO failed to configure the generated-objects directory",
        )
    }

    fn set_cache_dir(&self, dir: &Path) -> Result<(), String> {
        let dir = c_string_path(dir, "ThinLTO cache directory")?;
        self.run_bool(
            unsafe { kern_thinlto_session_set_cache_dir(self.0, dir.as_ptr()) },
            "LLVM ThinLTO failed to configure the cache directory",
        )
    }

    fn add_module(&self, module: &ThinLtoModule) -> Result<(), String> {
        let identifier = CString::new(module.identifier.as_str()).map_err(|_| {
            format!(
                "ThinLTO module identifier contains an interior NUL byte: {:?}",
                module.identifier
            )
        })?;
        self.run_bool(
            unsafe {
                kern_thinlto_session_add_module(
                    self.0,
                    identifier.as_ptr(),
                    module.bitcode.as_ptr(),
                    module.bitcode.len(),
                )
            },
            format!("LLVM ThinLTO could not add module `{}`", module.identifier),
        )
    }

    fn process(&self) -> Result<(), String> {
        self.run_bool(
            unsafe { kern_thinlto_session_process(self.0) },
            "LLVM ThinLTO failed during post-link processing",
        )
    }

    fn object_count(&self) -> usize {
        unsafe { kern_thinlto_session_object_count(self.0) }
    }

    fn object(&self, index: usize) -> Result<ThinLtoObject, String> {
        if unsafe { kern_thinlto_session_object_is_file(self.0, index) } != 0 {
            let path_len = unsafe { kern_thinlto_session_object_path_len(self.0, index) };
            if path_len == 0 {
                return Err(format!(
                    "LLVM ThinLTO returned an empty object-file path for output #{index}"
                ));
            }
            let mut path_bytes = vec![0u8; path_len];
            let copied = unsafe {
                kern_thinlto_session_copy_object_path(
                    self.0,
                    index,
                    path_bytes.as_mut_ptr() as *mut c_char,
                    path_bytes.len(),
                )
            };
            if copied == 0 {
                return Err(format!(
                    "LLVM ThinLTO failed to copy the object-file path for output #{index}"
                ));
            }
            return Ok(ThinLtoObject::File(PathBuf::from(
                String::from_utf8_lossy(&path_bytes).into_owned(),
            )));
        }

        let buffer_len = unsafe { kern_thinlto_session_object_buffer_len(self.0, index) };
        if buffer_len == 0 {
            return Err(format!(
                "LLVM ThinLTO returned an empty object buffer for output #{index}"
            ));
        }
        let mut buffer = vec![0u8; buffer_len];
        let copied = unsafe {
            kern_thinlto_session_copy_object_buffer(
                self.0,
                index,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        if copied == 0 {
            return Err(format!(
                "LLVM ThinLTO failed to copy the object buffer for output #{index}"
            ));
        }
        Ok(ThinLtoObject::Buffer(buffer))
    }

    fn run_bool(&self, status: i32, fallback: impl Into<String>) -> Result<(), String> {
        if status != 0 {
            Ok(())
        } else {
            Err(self.last_error(fallback.into()))
        }
    }

    fn last_error(&self, fallback: String) -> String {
        let message = unsafe { kern_thinlto_session_last_error(self.0) };
        if message.is_null() {
            fallback
        } else {
            let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
            if message.is_empty() {
                fallback
            } else {
                message.into_owned()
            }
        }
    }
}

impl Drop for ThinLtoSession {
    fn drop(&mut self) {
        unsafe { kern_thinlto_session_dispose(self.0) };
    }
}

fn c_string_path(path: &Path, label: &str) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|_| {
        format!(
            "{label} contains an interior NUL byte: `{}`",
            path.display()
        )
    })
}

#[repr(C)]
struct ThinLtoSessionOpaque {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn kern_thinlto_session_create() -> *mut ThinLtoSessionOpaque;
    fn kern_thinlto_session_dispose(session: *mut ThinLtoSessionOpaque);
    fn kern_thinlto_session_set_cpu(session: *mut ThinLtoSessionOpaque, cpu: *const c_char) -> i32;
    fn kern_thinlto_session_set_generated_objects_dir(
        session: *mut ThinLtoSessionOpaque,
        dir: *const c_char,
    ) -> i32;
    fn kern_thinlto_session_set_cache_dir(
        session: *mut ThinLtoSessionOpaque,
        dir: *const c_char,
    ) -> i32;
    fn kern_thinlto_session_add_module(
        session: *mut ThinLtoSessionOpaque,
        identifier: *const c_char,
        bitcode: *const u8,
        len: usize,
    ) -> i32;
    fn kern_thinlto_session_process(session: *mut ThinLtoSessionOpaque) -> i32;
    fn kern_thinlto_session_object_count(session: *const ThinLtoSessionOpaque) -> usize;
    fn kern_thinlto_session_object_is_file(
        session: *const ThinLtoSessionOpaque,
        index: usize,
    ) -> i32;
    fn kern_thinlto_session_object_path_len(
        session: *const ThinLtoSessionOpaque,
        index: usize,
    ) -> usize;
    fn kern_thinlto_session_copy_object_path(
        session: *const ThinLtoSessionOpaque,
        index: usize,
        dest: *mut c_char,
        dest_len: usize,
    ) -> i32;
    fn kern_thinlto_session_object_buffer_len(
        session: *const ThinLtoSessionOpaque,
        index: usize,
    ) -> usize;
    fn kern_thinlto_session_copy_object_buffer(
        session: *const ThinLtoSessionOpaque,
        index: usize,
        dest: *mut u8,
        dest_len: usize,
    ) -> i32;
    fn kern_thinlto_session_last_error(session: *const ThinLtoSessionOpaque) -> *const c_char;
}
