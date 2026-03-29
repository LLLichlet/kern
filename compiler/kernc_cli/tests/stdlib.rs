use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file_name = format!("{}_{}_{}.{}", prefix, std::process::id(), nanos, extension);
    std::env::temp_dir().join(file_name)
}

fn run_kernc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kernc"))
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn compiles_std_hello_world_in_compile_only_mode() {
    let source = repo_root().join("examples/hello_world.kr");
    let object = unique_temp_path("kernc_std_hello_world", "o");

    let source_arg = source.to_string_lossy().into_owned();
    let object_arg = object.to_string_lossy().into_owned();
    let args = vec![
        "-c",
        "--use-std",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ];
    let output = run_kernc(&args);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        object.exists(),
        "expected object file at {}",
        object.display()
    );

    let _ = fs::remove_file(&object);
}

#[test]
fn links_hosted_program_with_std_without_kern_entry_shims() {
    let source_path = unique_temp_path("kernc_std_hosted", "kr");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_hosted", exe_ext);

    fs::write(
        &source_path,
        r#"
use std.io;

extern fn main() i32 {
    io.println("hosted std", .{});
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let args = vec![
        "--use-std",
        "--link-profile",
        "hosted",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ];
    let output = run_kernc(&args);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        executable_path.exists(),
        "expected executable at {}",
        executable_path.display()
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_hosted_program_using_std_env_get() {
    let source_path = unique_temp_path("kernc_std_env", "kr");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_env", exe_ext);

    fs::write(
        &source_path,
        r#"
use std.env;
use std.mem.alloc.{PageAllocator, GPAllocator};

extern fn main() i32 {
    let page = PageAllocator.{}..&;
    let gpa = GPAllocator.{ backing: page }..&;

    if (!env.has(gpa, "KERN_STD_ENV_TEST")) {
        return 10;
    }
    if (env.has(gpa, "KERN_STD_ENV_MISSING")) {
        return 11;
    }

    let mut found = match (env.get(gpa, "KERN_STD_ENV_TEST")) {
        .Some: value => value,
        .None => return 1,
    };
    defer found..&.deinit(gpa);

    if (!found.&.eq("alpha-beta")) {
        return 2;
    }

    if (env.get(gpa, "KERN_STD_ENV_MISSING").is_some()) {
        return 3;
    }

    let mut fallback = match (env.get_or_clone(gpa, "KERN_STD_ENV_MISSING", "fallback")) {
        .Some: value => value,
        .None => return 4,
    };
    defer fallback..&.deinit(gpa);
    if (!fallback.&.eq("fallback")) {
        return 5;
    }

    let mut empty = match (env.get_or_empty(gpa, "KERN_STD_ENV_MISSING")) {
        .Some: value => value,
        .None => return 6,
    };
    defer empty..&.deinit(gpa);
    if (!empty.&.is_empty()) {
        return 7;
    }

    let mut saw_target = false;
    let visited = env.visit(.[saw_target = saw_target..&](entry: env.Var) bool {
        if (entry.name.eq("KERN_STD_ENV_TEST")) {
            if (!entry.value.eq("alpha-beta")) {
                return false;
            }
            saw_target.* = true;
        }
        return true;
    });
    if (visited == 0) {
        return 8;
    }
    if (!saw_target) {
        return 9;
    }

    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let args = vec![
        "--use-std",
        "--link-profile",
        "hosted",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ];
    let output = run_kernc(&args);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run_output = Command::new(&executable_path)
        .env("KERN_STD_ENV_TEST", "alpha-beta")
        .output()
        .unwrap();
    assert!(
        run_output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}
