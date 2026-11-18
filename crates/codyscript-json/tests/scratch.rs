//! A scratch pad for debugging the core natively — in RustRover, say, with
//! breakpoints in `codyscript-core` — rather than over WebAssembly in the
//! playground.
//!
//! Paste a program and its run configuration (the playground's Program and
//! Run configuration boxes) into `PROGRAM` and `CONFIG`, then debug
//! `scratch`. It prints the outcome as the playground shows it; it fails
//! only if `CONFIG` isn't a run configuration.
//!
//! It has the playground's demo executors, in Rust (see `executors`), so the
//! playground's cases run here too. With breakpoints in `CONFIG` it runs
//! under a debugger that prints each pause and continues; without, it's a
//! plain run. With `TRACE`, it pauses at every step from the first, and
//! prints each: a trace of the whole run, with the variables.
//!
//! To stop in Rust at a CodyScript step, give the step a `tag`, put `{
//! "tag": … }` in `CONFIG`'s breakpoints, and set a Rust breakpoint in the
//! debugger closure below. Step out of it (into `Session::before`, then the
//! run loop in `run.rs`) to follow the engine from there.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use codyscript_core::debug::{self, Action};
use codyscript_core::{Engine, RunConfig, RunOptions, Value, value};
use codyscript_json::EngineJsonExt;

const PROGRAM: &str = r#"{
  "ext": { "text": ["trim"], "pair": ["x"] },
  "do": [
    { "set": { "name": { "call": { "text.trim": { "value": { "ref": "raw" } } } } }, "tag": "trim" },
    { "set": { "fee": { "x": "amount * 0.03" } } },
    { "if": { "x": "fee > 300" }, "do": [{ "set": { "fee": 300 } }] },
    { "ret": { "call": { "pair.x": { "name": { "ref": "name" }, "fee": { "ref": "fee" } } } } }
  ]
}"#;

/// Pause at every step, printing each.
const TRACE: bool = false;

const CONFIG: &str = r#"{
  "variables": { "raw": "  Ada  ", "amount": 20000 },
  "breakpoints": [{ "tag": "trim" }]
}"#;

#[test]
fn scratch() {
    let config: RunConfig = codyscript_json::from_str(CONFIG).expect("CONFIG");
    let outcome = executors().compile_json(PROGRAM).and_then(|program| {
        let mut breakpoints = config.breakpoints;
        if TRACE {
            breakpoints.extend(program.steps().first().map(|step| step.breakpoint()));
        }
        let mut options = RunOptions::from(config.variables);
        if !breakpoints.is_empty() {
            let debugger = debug::from_fn(|pause| {
                if TRACE {
                    let step = pause.step();
                    let (steps, path, kind) = (pause.steps(), step.path, step.kind.to_string());
                    let name = step.name.unwrap_or("");
                    let scopes = codyscript_json::to_string(pause.scopes()).unwrap();
                    println!("{steps:>4}  {kind:<4} {name:<16} {path:<36} {scopes}");
                    return Action::StepInto;
                }
                println!("paused: {}", codyscript_json::to_string(&pause).unwrap());
                Action::Continue
            });
            options = options.with_debugger(breakpoints, debugger);
        }
        program.run_blocking(options)
    });
    match outcome {
        Ok(result) => println!(
            "ok: {}",
            codyscript_json::to_string_pretty(&result).unwrap()
        ),
        Err(error) => println!(
            "error: {}",
            codyscript_json::to_string_pretty(&error).unwrap()
        ),
    }
}

/// The playground's demo executors (`index.html`, `executors()`), in Rust.
/// `slow.wait` doesn't wait: it only counts, so `order` is the call's.
fn executors() -> Engine {
    let order = Arc::new(AtomicU64::new(0));
    Engine::builder()
        .executor("text", ["trim", "empty"], |call| {
            let text = call
                .input
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("");
            Ok(match call.op.as_str() {
                "trim" => text.trim().into(),
                _ => text.is_empty().into(),
            })
        })
        .executor("slow", ["wait"], move |call| {
            let id = call.arg.as_ref().and_then(|arg| arg.get("id")).cloned();
            let order = order.fetch_add(1, Ordering::Relaxed) + 1;
            Ok(value!({ "id": id, "order": order }))
        })
        .executor("pair", ["x"], |call| Ok(Value::Object(call.input)))
        .executor("fail", ["throw", "reject"], |call| {
            Err(match call.arg {
                Some(Value::String(message)) => message.into(),
                arg => codyscript_json::to_string(&arg.unwrap_or(Value::Null))
                    .unwrap()
                    .into(),
            })
        })
        .build()
}
