use shared_cli::{ColorChoice, HelpDoc, HelpSection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpTopic {
    Overview,
    All,
}

pub fn render_help(program_name: &str, topic: HelpTopic, color: ColorChoice) -> String {
    let doc = match topic {
        HelpTopic::Overview => overview_doc(program_name),
        HelpTopic::All => all_doc(program_name),
    };
    doc.render(color)
}

pub fn version_text() -> String {
    format!("Kern Compiler v{}", env!("CARGO_PKG_VERSION"))
}

fn overview_doc(program_name: &str) -> HelpDoc {
    HelpDoc::new(version_text())
        .summary("Compile Kern source files, emit LLVM IR, or drive the final system link step")
        .usage(format!("{program_name} [OPTIONS] <input.rn>"))
        .usage(format!("{program_name} -c [OPTIONS] <input.rn>"))
        .usage(format!("{program_name} --cc [OPTIONS] <input.c>"))
        .usage(format!("{program_name} --link-only [OPTIONS]"))
        .usage(format!("{program_name} help all"))
        .section(
            HelpSection::new("Common Options")
                .entry(
                    "-o <FILE>",
                    "Write the final artifact or object file to <FILE>",
                )
                .entry(
                    "-c",
                    "Emit linker input and skip the final system link step",
                )
                .entry(
                    "--cc",
                    "Compile a C-family source to a native object with the resolved C compiler",
                )
                .entry(
                    "--link-only",
                    "Skip frontend/codegen and invoke the linker driver only",
                )
                .entry("-O0 ... -O3", "Select the optimization level")
                .entry("-g / -g0", "Enable or disable debug info emission")
                .entry("--target <TRIPLE>", "Select the target triple")
                .entry(
                    "--library-bundle <BUNDLE>",
                    "Select official library aliases: none, base, std\nDefault direct source builds use `std`",
                ),
        )
        .section(
            HelpSection::new("Module And Metadata Inputs")
                .entry(
                    "--define <KEY=VALUE>",
                    "Define a conditional compilation symbol",
                )
                .entry(
                    "--module-path <NAME=PATH>",
                    "Map a source module alias to a directory",
                )
                .entry(
                    "--module-interface-path <NAME=PATH>",
                    "Map an imported metadata module alias to a root directory",
                )
                .entry(
                    "--metadata-output <DIR>",
                    "Write module metadata snapshots to <DIR>",
                )
                .entry(
                    "--test-mode",
                    "Compile a test target, collect #[test] cases, and enable #[if(test)]",
                )
                .entry(
                    "--test-metadata-output <FILE>",
                    "Write discovered test case metadata to <FILE>",
                )
                .entry(
                    "--module-root-name <NAME>",
                    "Override the compiled root module name",
                ),
        )
        .section(
            HelpSection::new("Diagnostics And Introspection")
                .entry(
                    "--emit-llvm[=STAGE]",
                    "Print LLVM IR instead of object code",
                )
                .entry(
                    "--timings",
                    "Print compiler phase timings and cache statistics",
                )
                .entry(
                    "--print-link-command",
                    "Print the resolved linker command before execution",
                )
                .entry("-h, --help", "Show the common help view")
                .entry("help all", "Show the full option reference"),
        )
        .example(
            format!("{program_name} hello.rn -o hello"),
            "Compile and link an executable",
        )
        .example(
            format!("{program_name} -c kernel/init.rn -O2 -g"),
            "Compile only and leave the final link to another step",
        )
        .example(
            format!("{program_name} --emit-llvm=optimized hello.rn"),
            "Inspect optimized LLVM IR",
        )
        .note("Use `kernc help all` for the full codegen, linker, and metadata option reference.")
}

fn all_doc(program_name: &str) -> HelpDoc {
    HelpDoc::new(format!("{} full help", version_text()))
        .summary("Complete option reference for the Kern compiler driver")
        .usage(format!("{program_name} [OPTIONS] <input.rn>"))
        .usage(format!("{program_name} -c [OPTIONS] <input.rn>"))
        .usage(format!("{program_name} --cc [OPTIONS] <input.c>"))
        .usage(format!("{program_name} --link-only [OPTIONS]"))
        .section(
            HelpSection::new("Build Options")
                .entry("-o <FILE>", "Write output to <FILE>")
                .entry(
                    "-c",
                    "Emit linker input and skip the final system link step",
                )
                .entry(
                    "--cc",
                    "Compile a C-family source to a native object with the resolved C compiler",
                )
                .entry(
                    "--link-only",
                    "Skip frontend/codegen and invoke the linker driver only",
                )
                .entry(
                    "--define <KEY=VALUE>",
                    "Define a conditional compilation symbol",
                )
                .entry(
                    "--module-path <NAME=PATH>",
                    "Map a source module alias to a directory",
                )
                .entry(
                    "--module-interface-path <NAME=PATH>",
                    "Map an imported metadata module alias to a root directory",
                )
                .entry(
                    "--metadata-output <DIR>",
                    "Write module metadata snapshots to <DIR>",
                )
                .entry(
                    "--test-mode",
                    "Compile a test target, collect #[test] cases, and enable #[if(test)]",
                )
                .entry(
                    "--test-metadata-output <FILE>",
                    "Write discovered test case metadata to <FILE>",
                )
                .entry(
                    "--module-root-name <NAME>",
                    "Override the compiled root module name",
                )
                .entry("-O0 ... -O3", "Set the optimization level")
                .entry("-g / -g0", "Enable or disable debug info emission"),
        )
        .section(
            HelpSection::new("Targeting And Codegen")
                .entry("--target <TRIPLE>", "Select the target triple")
                .entry(
                    "--asm-dialect <DIALECT>",
                    "Assembly dialect: auto, intel, att",
                )
                .entry("--codegen-units <N>", "Split codegen into N lowered units")
                .entry(
                    "--lto <MODE>",
                    "Cross-CGU optimization mode: none, full, thin",
                )
                .entry(
                    "--code-model <MODEL>",
                    "LLVM code model: default, small, kernel, medium, large",
                )
                .entry("--debug-info <YES|NO>", "Whether to emit debug info")
                .entry(
                    "--emit-llvm[=STAGE]",
                    "Print LLVM IR stage: raw, verified, optimized",
                )
                .entry(
                    "--timings",
                    "Print compiler phase timings and cache statistics",
                ),
        )
        .section(
            HelpSection::new("Linking And Runtime")
                .entry(
                    "--toolchain-root <DIR>",
                    "Prefer toolchain binaries under <DIR>",
                )
                .entry(
                    "--link-driver <CMD>",
                    "Explicitly use an external linker driver command",
                )
                .entry(
                    "--runtime-entry <MODE>",
                    "Runtime entry contract: none, rt, crt\nDefault direct source builds use `rt`",
                )
                .entry("--runtime-libc <YES|NO>", "Link libc: yes or no")
                .entry(
                    "--library-bundle <BUNDLE>",
                    "Select official library aliases: none, base, std\nDefault direct source builds use `std`",
                )
                .entry("--link-input <PATH>", "Add a linker input path")
                .entry("--link-search <DIR>", "Add a linker search path")
                .entry("--link-lib <NAME>", "Link against a library")
                .entry("-L <DIR>", "Add a linker search path")
                .entry("-l <NAME>", "Link against a library")
                .entry(
                    "--link-arg <ARG>",
                    "Pass a raw argument through to the linker driver",
                )
                .entry(
                    "--cc-arg <ARG>",
                    "Pass a raw argument through to `--cc` C-family compilation",
                )
                .entry(
                    "--entry-symbol <SYMBOL>",
                    "Override the default kernc entry symbol",
                )
                .entry(
                    "--print-link-command",
                    "Print the resolved linker command before execution",
                ),
        )
        .section(
            HelpSection::new("Information")
                .entry("-v, -V, --version", "Show version information and exit")
                .entry("-h, --help", "Show the common help view and exit")
                .entry("help all", "Show this full help view"),
        )
        .example(
            format!("{program_name} hello.rn -o hello"),
            "Compile and link an executable",
        )
        .example(
            format!("{program_name} --link-only --link-input hello.o -l c"),
            "Run only the final link step",
        )
        .example(
            format!("{program_name} --module-path std=./library/std main.rn"),
            "Build with an explicit source module alias",
        )
}
