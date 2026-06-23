//! Bridge entry point. Slack Bolt (Socket Mode) wiring is not ported yet; the
//! behavior core in the library is still stubbed (see `cargo test` / the spec
//! suite). This binary exists so the crate builds and the port has a `main` to
//! grow into.

fn main() {
    eprintln!(
        "slack-claude-bridge (Rust port): not yet implemented — the behavior layer is stubbed.\n\
         Run `cargo test` to see the spec suite (it runs every scenario from ../spec/scenarios.json, currently red).\n\
         See rust/README.md."
    );
    std::process::exit(1);
}
