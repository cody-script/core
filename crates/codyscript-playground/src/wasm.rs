//! Builds the module the page loads: `codyscript-wasm` compiled to
//! `wasm32-unknown-unknown`, plus the JavaScript glue `wasm-bindgen`
//! generates for it — the same as `cargo build --target …` followed by the
//! `wasm-bindgen --target web` CLI, but with no CLI to install and nothing
//! written outside `target/`.

use std::path::PathBuf;
use std::process::Command;

use wasm_bindgen_cli_support::Bindgen;

/// Which build of the module to serve.
#[derive(Clone, Copy, Debug)]
pub enum Profile {
    Debug,
    Release,
}

/// The module, ready to serve.
pub struct Module {
    /// `codyscript_wasm.js`: the ES module the page imports.
    pub js: String,
    /// `codyscript_wasm_bg.wasm`: what the glue loads.
    pub wasm: Vec<u8>,
}

pub fn build(profile: Profile) -> Result<Module, String> {
    let path = compile(profile)?;
    let mut output = Bindgen::new()
        .input_path(&path)
        .web(true)
        .and_then(|bindgen| bindgen.typescript(false).generate_output())
        .map_err(|e| format!("generating the JavaScript glue: {e:#}"))?;
    // Inline JavaScript (`#[wasm_bindgen(inline_js)]`) would need serving
    // too; this crate has none.
    let has_snippets = output
        .snippets()
        .values()
        .any(|snippets| !snippets.is_empty());
    if has_snippets || !output.local_modules().is_empty() {
        return Err(
            "the glue needs extra JavaScript files, which the playground doesn't serve".into(),
        );
    }
    Ok(Module {
        js: output.js().to_owned(),
        wasm: output.wasm_mut().emit_wasm(),
    })
}

/// Runs `cargo build` for `codyscript-wasm` on wasm32; returns the `.wasm`
/// file.
fn compile(profile: Profile) -> Result<PathBuf, String> {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Its own target directory, so this build never waits on (or for) the
    // `cargo run` that started the playground.
    let target_dir = workspace.join("target/playground");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(&workspace)
        .args(["build", "-p", "codyscript-wasm", "--lib"])
        .args(["--target", "wasm32-unknown-unknown", "--target-dir"])
        .arg(&target_dir);
    if let Profile::Release = profile {
        command.arg("--release");
    }
    let status = command
        .status()
        .map_err(|e| format!("running cargo: {e}"))?;
    if !status.success() {
        return Err("compiling to WebAssembly failed (is the target installed? \
                    `rustup target add wasm32-unknown-unknown`)"
            .into());
    }
    let profile_dir = match profile {
        Profile::Debug => "debug",
        Profile::Release => "release",
    };
    Ok(target_dir
        .join("wasm32-unknown-unknown")
        .join(profile_dir)
        .join("codyscript_wasm.wasm"))
}
