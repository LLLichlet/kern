//! Minimal terminal rendering helpers shared by first-party command-line tools.
//!
//! The types here intentionally avoid command-specific policy.  Callers build
//! structured help or error reports, and this module handles color selection and
//! deterministic text layout.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HelpDoc {
    title: String,
    summary: Option<String>,
    usages: Vec<String>,
    sections: Vec<HelpSection>,
    examples: Vec<HelpExample>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ErrorReport {
    title: String,
    message: String,
    sections: Vec<ErrorSection>,
    hints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    title: String,
    items: Vec<HelpItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpExample {
    command: String,
    description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HelpItem {
    Entry { label: String, description: String },
    Paragraph(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ErrorSection {
    title: String,
    body: String,
}

impl HelpDoc {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    pub fn usage(mut self, usage: impl Into<String>) -> Self {
        self.usages.push(usage.into());
        self
    }

    pub fn section(mut self, section: HelpSection) -> Self {
        self.sections.push(section);
        self
    }

    pub fn example(mut self, command: impl Into<String>, description: impl Into<String>) -> Self {
        self.examples.push(HelpExample {
            command: command.into(),
            description: Some(description.into()),
        });
        self
    }

    pub fn example_only(mut self, command: impl Into<String>) -> Self {
        self.examples.push(HelpExample {
            command: command.into(),
            description: None,
        });
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn render(&self, color: ColorChoice) -> String {
        let palette = Palette::stdout(color);
        let mut out = String::new();

        push_line(&mut out, &palette.heading(&self.title));
        if let Some(summary) = &self.summary {
            push_line(&mut out, &palette.subtle(summary));
        }

        if !self.usages.is_empty() {
            out.push('\n');
            push_line(&mut out, &palette.section("Usage"));
            for usage in &self.usages {
                push_line(&mut out, &format!("  {}", palette.usage(usage)));
            }
        }

        for section in &self.sections {
            out.push('\n');
            push_line(&mut out, &palette.section(&section.title));
            render_section(&mut out, section, &palette);
        }

        if !self.examples.is_empty() {
            out.push('\n');
            push_line(&mut out, &palette.section("Examples"));
            for example in &self.examples {
                push_line(
                    &mut out,
                    &format!("  {}", palette.example(&example.command)),
                );
                if let Some(description) = &example.description {
                    push_line(&mut out, &format!("      {}", palette.subtle(description)));
                }
            }
        }

        if !self.notes.is_empty() {
            out.push('\n');
            push_line(&mut out, &palette.section("Notes"));
            for note in &self.notes {
                push_line(&mut out, &format!("  {}", palette.note(note)));
            }
        }

        out
    }
}

impl ErrorReport {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            ..Self::default()
        }
    }

    pub fn section(mut self, title: impl Into<String>, body: impl Into<String>) -> Self {
        self.sections.push(ErrorSection {
            title: title.into(),
            body: body.into(),
        });
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    pub fn render(&self, color: ColorChoice) -> String {
        let palette = Palette::stderr(color);
        let mut out = String::new();

        push_line(&mut out, &palette.error_heading(&self.title));
        for line in self.message.lines() {
            push_line(&mut out, &format!("  {line}"));
        }

        for section in &self.sections {
            out.push('\n');
            push_line(&mut out, &palette.section(&section.title));
            for line in section.body.lines() {
                push_line(&mut out, &format!("  {line}"));
            }
        }

        if !self.hints.is_empty() {
            out.push('\n');
            push_line(&mut out, &palette.hint_heading("Hint"));
            for hint in &self.hints {
                push_line(&mut out, &format!("  {}", palette.hint(hint)));
            }
        }

        out
    }
}

impl HelpSection {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
        }
    }

    pub fn entry(mut self, label: impl Into<String>, description: impl Into<String>) -> Self {
        self.items.push(HelpItem::Entry {
            label: label.into(),
            description: description.into(),
        });
        self
    }

    pub fn paragraph(mut self, text: impl Into<String>) -> Self {
        self.items.push(HelpItem::Paragraph(text.into()));
        self
    }
}

fn render_section(out: &mut String, section: &HelpSection, palette: &Palette) {
    let label_width = section
        .items
        .iter()
        .filter_map(|item| match item {
            HelpItem::Entry { label, .. } => Some(label.len()),
            HelpItem::Paragraph(_) => None,
        })
        .max()
        .unwrap_or(0)
        .clamp(0, 28);

    for item in &section.items {
        match item {
            HelpItem::Entry { label, description } => {
                let indent = if label_width == 0 { 2 } else { label_width + 4 };
                let mut lines = description.lines();
                let first = lines.next().unwrap_or_default();
                if label.len() > label_width {
                    push_line(out, &format!("  {}", palette.label(label)));
                    push_line(out, &format!("{:indent$}{first}", "", indent = indent));
                } else {
                    push_line(
                        out,
                        &format!(
                            "  {:width$}  {}",
                            palette.label(label),
                            first,
                            width = label_width
                        ),
                    );
                }
                for line in lines {
                    push_line(out, &format!("{:indent$}{line}", "", indent = indent));
                }
            }
            HelpItem::Paragraph(text) => {
                for line in text.lines() {
                    push_line(out, &format!("  {line}"));
                }
            }
        }
    }
}

fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

struct Palette {
    enabled: bool,
}

#[derive(Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

impl Palette {
    fn stdout(color: ColorChoice) -> Self {
        Self::new(color, OutputStream::Stdout)
    }

    fn stderr(color: ColorChoice) -> Self {
        Self::new(color, OutputStream::Stderr)
    }

    fn new(color: ColorChoice, stream: OutputStream) -> Self {
        let enabled = match color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                let terminal = match stream {
                    OutputStream::Stdout => std::io::stdout().is_terminal(),
                    OutputStream::Stderr => std::io::stderr().is_terminal(),
                };
                terminal && std::env::var_os("NO_COLOR").is_none()
            }
        };
        Self { enabled }
    }

    fn heading(&self, text: &str) -> String {
        self.paint("1;36", text)
    }

    fn section(&self, text: &str) -> String {
        self.paint("1;37", text)
    }

    fn label(&self, text: &str) -> String {
        self.paint("1;36", text)
    }

    fn subtle(&self, text: &str) -> String {
        self.paint("2", text)
    }

    fn usage(&self, text: &str) -> String {
        self.paint("32", text)
    }

    fn example(&self, text: &str) -> String {
        self.paint("1;32", text)
    }

    fn note(&self, text: &str) -> String {
        self.paint("33", text)
    }

    fn error_heading(&self, text: &str) -> String {
        self.paint("1;31", text)
    }

    fn hint_heading(&self, text: &str) -> String {
        self.paint("1;33", text)
    }

    fn hint(&self, text: &str) -> String {
        self.paint("33", text)
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_entries_and_examples_without_color() {
        let doc = HelpDoc::new("Tool v1.0")
            .summary("Short summary")
            .usage("tool [OPTIONS]")
            .section(
                HelpSection::new("Commands")
                    .entry("build", "Compile the selected target")
                    .entry("run", "Compile and execute the default binary"),
            )
            .example("tool build", "Build the default target")
            .note("Use `tool help build` for more detail.");

        let rendered = doc.render(ColorChoice::Never);
        assert!(rendered.contains("Tool v1.0"));
        assert!(rendered.contains("Commands"));
        assert!(rendered.contains("build"));
        assert!(rendered.contains("tool build"));
        assert!(rendered.contains("Use `tool help build`"));
    }

    #[test]
    fn renders_help_with_color_when_forced() {
        let doc = HelpDoc::new("Tool v1.0")
            .summary("Short summary")
            .usage("tool [OPTIONS]")
            .note("Run `tool help build` for more detail.");

        let rendered = doc.render(ColorChoice::Always);
        assert!(rendered.contains("\x1b[1;36mTool v1.0\x1b[0m"));
        assert!(rendered.contains("\x1b[32mtool [OPTIONS]\x1b[0m"));
        assert!(rendered.contains("\x1b[33mRun `tool help build` for more detail.\x1b[0m"));
    }

    #[test]
    fn renders_error_reports_with_sections_and_hints() {
        let report = ErrorReport::new("tool error", "unsupported argument `--wat`")
            .section("Usage", "tool [OPTIONS]")
            .hint("Run `tool --help` for the full reference.");

        let rendered = report.render(ColorChoice::Never);
        assert!(rendered.contains("tool error"));
        assert!(rendered.contains("unsupported argument `--wat`"));
        assert!(rendered.contains("Usage"));
        assert!(rendered.contains("tool [OPTIONS]"));
        assert!(rendered.contains("Hint"));
        assert!(rendered.contains("Run `tool --help` for the full reference."));
    }
}
