use shared_cli::{ColorChoice, HelpDoc, HelpSection};

pub fn render_help(color: ColorChoice) -> String {
    HelpDoc::new(version_text())
        .summary("Language server for Kern source and package analysis")
        .usage("kern-lsp [OPTIONS]")
        .usage("kern-lsp help")
        .section(
            HelpSection::new("Analysis Options")
                .entry(
                    "--library-bundle <BUNDLE>",
                    "Select official library aliases for analysis: none, base, std",
                )
                .entry(
                    "--features <A,B>",
                    "Enable a comma-separated feature list for project analysis",
                )
                .entry(
                    "--no-default-features",
                    "Disable the implicit `default` feature during analysis",
                )
                .entry(
                    "--module-path <NAME=PATH>",
                    "Map a source module alias to a directory",
                )
                .entry(
                    "--module-interface-path <NAME=PATH>",
                    "Map an imported metadata module alias to a root directory",
                ),
        )
        .section(HelpSection::new("Server Options").entry(
            "--worker-threads <N>",
            "Set the bounded worker pool size for LSP analysis tasks",
        ))
        .section(
            HelpSection::new("Information")
                .entry("-v, -V, --version", "Show version information and exit")
                .entry("-h, --help", "Show this help text and exit"),
        )
        .example(
            "kern-lsp",
            "Run the server with the default `std` library bundle",
        )
        .example(
            "kern-lsp --no-default-features --features tls,simd",
            "Analyze a project with an explicit feature set",
        )
        .note("The server speaks LSP over stdin/stdout and is normally launched by an editor.")
        .render(color)
}

pub fn version_text() -> String {
    format!("Kern Language Server v{}", env!("CARGO_PKG_VERSION"))
}
