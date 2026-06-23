//! Tiny `--help` / `--version` handling shared by the binaries, so the crate
//! stays clap-free.
//!
//! Only the *first* CLI argument is inspected: these tools have no subcommands,
//! and `slack-claude-job` takes a free-form prompt where scanning every argument
//! for `--help` would clash with prompt text (e.g. `slack-claude-job "what does
//! --help do"`).

/// Handle `-h`/`--help` and `-V`/`--version` if they are the first argument.
///
/// Prints to stdout and exits 0 on a match; otherwise returns so the caller can
/// continue parsing. `usage` is the multi-line help body shown after the
/// `<bin> <version>` header.
pub fn handle_help_version(bin: &str, usage: &str) {
    let Some(first) = std::env::args().nth(1) else {
        return;
    };
    match first.as_str() {
        "-h" | "--help" => {
            println!("{bin} {}\n\n{usage}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        "-V" | "--version" => {
            println!("{bin} {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        _ => {}
    }
}
