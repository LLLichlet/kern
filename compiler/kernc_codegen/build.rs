use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/thinlto_bridge.cpp");

    let llvm_libdir = llvm_dependency_var("LIBDIR")
        .expect("llvm-sys did not provide DEP_LLVM_*_LIBDIR to kernc_codegen");
    let llvm_includedir = llvm_include_dir(&llvm_libdir);

    let mut build = cc::Build::new();
    build.cpp(true);
    build.static_crt(true);
    build.file("src/thinlto_bridge.cpp");
    add_llvm_include_dir(&mut build, &llvm_includedir);
    build.flag_if_supported("-std=c++17");
    build.flag_if_supported("/std:c++17");
    build.flag_if_supported("/EHsc");
    build.compile("kern_thinlto_bridge");

    println!("cargo:rustc-link-search=native={llvm_libdir}");
}

fn llvm_dependency_var(suffix: &str) -> Option<String> {
    env::vars().find_map(|(key, value)| {
        if key.starts_with("DEP_LLVM_") && key.ends_with(suffix) {
            Some(value)
        } else {
            None
        }
    })
}

fn llvm_include_dir(llvm_libdir: &str) -> String {
    let libdir = PathBuf::from(llvm_libdir);
    if let Some(prefix) = libdir.parent() {
        let include = prefix.join("include");
        if include.is_dir() {
            return include.to_string_lossy().into_owned();
        }
    }

    if let Some(llvm_config) = llvm_dependency_var("CONFIG_PATH") {
        let output = Command::new(&llvm_config)
            .arg("--includedir")
            .output()
            .unwrap_or_else(|err| panic!("failed to run `{llvm_config} --includedir`: {err}"));
        if output.status.success() {
            return String::from_utf8(output.stdout)
                .expect("llvm-config output was not valid UTF-8")
                .trim()
                .to_string();
        }
    }

    panic!("failed to infer the LLVM include directory from `{llvm_libdir}`");
}

fn add_llvm_include_dir(build: &mut cc::Build, llvm_includedir: &str) {
    let compiler = build.get_compiler();

    if compiler.is_like_msvc() {
        build.flag_if_supported("/experimental:external");
        build.flag_if_supported("/external:W0");
        build.flag(&format!("/external:I{llvm_includedir}"));
        return;
    }

    build.flag("-isystem");
    build.flag(llvm_includedir);
}
