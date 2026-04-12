use crate::plan::TargetKind;
use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};

pub(crate) fn apply_target_runtime_defaults(options: &mut CompileOptions, target_kind: TargetKind) {
    match target_kind {
        TargetKind::Lib => {
            options.runtime_entry = RuntimeEntry::None;
            options.runtime_libc = false;
            options.library_bundle = LibraryBundle::Std;
        }
        TargetKind::Bin | TargetKind::Example => {
            options.runtime_entry = RuntimeEntry::Crt;
            options.runtime_libc = true;
            options.library_bundle = LibraryBundle::Std;
        }
        TargetKind::Test => {
            options.runtime_entry = RuntimeEntry::Rt;
            options.runtime_libc = false;
            options.library_bundle = LibraryBundle::Std;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply_target_runtime_defaults;
    use crate::plan::TargetKind;
    use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};

    #[test]
    fn lib_targets_use_library_runtime_defaults() {
        let mut options = CompileOptions::default();
        apply_target_runtime_defaults(&mut options, TargetKind::Lib);

        assert_eq!(options.runtime_entry, RuntimeEntry::None);
        assert!(!options.runtime_libc);
        assert_eq!(options.library_bundle, LibraryBundle::Std);
    }

    #[test]
    fn hosted_executable_targets_use_hosted_runtime_defaults() {
        for target_kind in [TargetKind::Bin, TargetKind::Example] {
            let mut options = CompileOptions::default();
            apply_target_runtime_defaults(&mut options, target_kind);

            assert_eq!(options.runtime_entry, RuntimeEntry::Crt);
            assert!(options.runtime_libc);
            assert_eq!(options.library_bundle, LibraryBundle::Std);
        }
    }

    #[test]
    fn test_targets_use_rt_without_libc_by_default() {
        let mut options = CompileOptions::default();
        apply_target_runtime_defaults(&mut options, TargetKind::Test);

        assert_eq!(options.runtime_entry, RuntimeEntry::Rt);
        assert!(!options.runtime_libc);
        assert_eq!(options.library_bundle, LibraryBundle::Std);
    }
}
