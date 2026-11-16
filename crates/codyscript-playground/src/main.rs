//! The CodyScript playground: a web page for smoke-testing `codyscript-wasm`
//! — CodyScript compiled to WebAssembly — in a real browser.
//!
//! ```text
//! rustup target add wasm32-unknown-unknown      # once
//! cargo run -p codyscript-playground            # then open http://127.0.0.1:8080
//! ```
//!
//! On start it compiles `codyscript-wasm` to WebAssembly and generates its
//! JavaScript glue ([`wasm`]), then serves, from memory ([`server`]):
//!
//! - `/` — the page (`index.html`): edit and run a program, or
//!   click **Run all** to run the checklist; every case should pass;
//! - `/cases.json` — the checklist ([`cases`]): programs and the outcomes to
//!   expect, written in Rust;
//! - `/pkg/…` — the module and its glue.
//!
//! Options: `--port <port>` (default 8080) and `--debug` (a debug build of
//! the module: faster to compile, slower to run).
//!
//! It's a separate crate so that its server dependencies never touch
//! `codyscript-wasm`. The automated counterpart is that crate's
//! `tests/web.rs` (`cargo test -p codyscript-wasm --target
//! wasm32-unknown-unknown`).

use std::process::ExitCode;

mod cases;
mod server;
mod wasm;

fn main() -> ExitCode {
    let (port, profile) = match arguments() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("error: {message}\nusage: playground [--port <port>] [--debug]");
            return ExitCode::from(2);
        }
    };
    println!("Compiling codyscript-wasm to WebAssembly ({profile:?})…");
    let module = match wasm::build(profile) {
        Ok(module) => module,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    match server::serve(port, module) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// `--port <port>` and `--debug`.
fn arguments() -> Result<(u16, wasm::Profile), String> {
    let (mut port, mut profile) = (8080, wasm::Profile::Release);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                let value = args.next().ok_or("--port needs a value")?;
                port = value.parse().map_err(|e| format!("--port {value}: {e}"))?;
            }
            "--debug" => profile = wasm::Profile::Debug,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok((port, profile))
}
