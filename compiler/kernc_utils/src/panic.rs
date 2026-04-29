use std::panic::{self, PanicHookInfo};
use std::sync::Once;

static INSTALL: Once = Once::new();

pub fn install_compiler_panic_hook(program_name: &'static str) {
    INSTALL.call_once(|| {
        panic::set_hook(Box::new(move |info| {
            eprintln!("ICE: Kern Compiler Internal Error");
            eprintln!("This is a bug in the compiler. Please report this issue at:");
            eprintln!("https://github.com/softfault/kern/issues");

            let message = panic_message(info);
            if !message.is_empty() {
                eprintln!("panic: {message}");
            }
            if let Some(location) = info.location() {
                eprintln!(
                    "location: {}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                );
            }
            eprintln!(
                "note: `{program_name}` suppressed the Rust panic backtrace; include the source that triggered this ICE in the report."
            );
        }));
    });
}

fn panic_message(info: &PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = info.payload().downcast_ref::<String>() {
        return message.clone();
    }
    String::new()
}
