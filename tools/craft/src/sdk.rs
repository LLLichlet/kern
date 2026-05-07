use std::path::{Path, PathBuf};

const SDK_ENV_VAR: &str = "KERN_CRAFT_SDK_ROOT";

pub(crate) fn sdk_root() -> PathBuf {
    sdk_root_from_current_exe().unwrap_or_else(source_sdk_root)
}

fn sdk_root_from_current_exe() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(SDK_ENV_VAR) {
        let candidate = PathBuf::from(path);
        if is_valid_sdk_root(&candidate) {
            return Some(candidate);
        }
    }

    let exe = std::env::current_exe().ok()?;
    sdk_root_for_executable(&exe)
}

fn source_sdk_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk")
}

fn is_valid_sdk_root(path: &Path) -> bool {
    path.join("init.rn").is_file()
        && path.join("builder.rn").is_file()
        && path.join("plan.rn").is_file()
}

pub(crate) fn sdk_root_for_executable(exe: &Path) -> Option<PathBuf> {
    for ancestor in exe.ancestors() {
        let candidate = ancestor.join("lib").join("kern").join("craft");
        if is_valid_sdk_root(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discovers_packaged_sdk_root_relative_to_binary() {
        let root = temp_dir("craft-sdk-root");
        let sdk = root.join("lib").join("kern").join("craft");
        fs::create_dir_all(&sdk).unwrap();
        fs::write(sdk.join("init.rn"), "pub mod builder;\npub mod plan;\n").unwrap();
        fs::write(sdk.join("builder.rn"), "pub struct Builder {};\n").unwrap();
        fs::write(sdk.join("plan.rn"), "pub struct Plan {};\n").unwrap();

        let exe = root.join("bin").join("craft");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, "").unwrap();

        assert_eq!(sdk_root_for_executable(&exe), Some(sdk));
    }

    #[test]
    fn source_tree_sdk_root_is_valid() {
        assert!(is_valid_sdk_root(&source_sdk_root()));
    }
}
