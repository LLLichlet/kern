//! Small LLVM bitcode file probes shared by the driver and linker pipeline.
//!
//! Kern sometimes has to distinguish object files from raw or wrapped LLVM
//! bitcode before handing linker inputs to LLVM's LTO path.  The check here is
//! intentionally only a magic-number probe; it does not validate the module.

use std::fs;
use std::path::Path;

pub const LLVM_RAW_BITCODE_MAGIC: [u8; 4] = *b"BC\xc0\xde";

// LLVM also supports a wrapped bitcode container whose header magic is
// 0x0B17C0DE. On disk that 32-bit value is stored little-endian, which is why
// the file starts with DE C0 17 0B instead of the source-order spelling.
pub const LLVM_WRAPPER_BITCODE_MAGIC: [u8; 4] = [0xDE, 0xC0, 0x17, 0x0B];

pub fn is_llvm_bitcode(bytes: &[u8]) -> bool {
    bytes.starts_with(&LLVM_RAW_BITCODE_MAGIC) || bytes.starts_with(&LLVM_WRAPPER_BITCODE_MAGIC)
}

pub fn file_has_llvm_bitcode_magic(path: &Path) -> bool {
    fs::read(path)
        .map(|bytes| is_llvm_bitcode(&bytes))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        LLVM_RAW_BITCODE_MAGIC, LLVM_WRAPPER_BITCODE_MAGIC, file_has_llvm_bitcode_magic,
        is_llvm_bitcode,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file_path(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}.bc"))
    }

    #[test]
    fn detects_raw_llvm_bitcode_magic() {
        assert!(is_llvm_bitcode(&LLVM_RAW_BITCODE_MAGIC));
    }

    #[test]
    fn detects_wrapped_llvm_bitcode_magic() {
        assert!(is_llvm_bitcode(&LLVM_WRAPPER_BITCODE_MAGIC));
    }

    #[test]
    fn detects_bitcode_magic_from_file() {
        let path = temp_file_path("kernc-utils-bitcode-magic");
        fs::write(&path, LLVM_WRAPPER_BITCODE_MAGIC).unwrap();
        assert!(file_has_llvm_bitcode_magic(&path));
        let _ = fs::remove_file(path);
    }
}
