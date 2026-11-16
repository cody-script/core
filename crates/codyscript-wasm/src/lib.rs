//! CodyScript in WebAssembly: JSON programs run from JavaScript, calling
//! executors written in JavaScript.
//!
//! ```js
//! import init, { run } from "./pkg/codyscript_wasm.js";
//! await init();
//! const executors = {
//!   text: ({ op, input }) => op === "trim" ? String(input.value ?? "").trim() : null,
//!   slow: async ({ arg }) => { await new Promise((r) => setTimeout(r, 10)); return arg; },
//! };
//! const outcome = JSON.parse(await run(programJson, '{"Name":" Ada "}', executors, '{"maxSteps":1000}'));
//! // { ok: true, value, variables, steps }  or  { ok: false, error: { code, message, path, tag?, issues? } }
//! ```
//!
//! Values cross the boundary as JSON text, so this exercises the whole stack
//! a host would use: `codyscript-json` parsing and printing, the core engine,
//! and async executors backed by JavaScript promises.

use codyscript_core::{
    BoxError, BoxFuture, Engine, Error, Executor, ExecutorCall, ExecutorResult, Limits, RunOptions,
    RunResult, Value, Variables,
};
use codyscript_json::EngineJsonExt;
use js_sys::{Array, Function, Object, Promise, JSON};
use send_wrapper::SendWrapper;
use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Shows Rust panics in the browser console instead of `unreachable`.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// The crate's version.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

/// The language's JSON Schema, as JSON text.
#[wasm_bindgen]
pub fn schema() -> String {
    codyscript_json::SCHEMA.into()
}

/// Compiles and runs a program.
///
/// - `program`: JSON text.
/// - `variables`: JSON text of an object, or `""` for none.
/// - `executors`: an object of functions, `{ id: (call) => value }`, or
///   `null`. Each receives `{ op, arg?, input }` and returns a JSON value, or
///   a promise of one; `undefined` is `null`, and throwing (or rejecting)
///   fails the run with `executor_failed`.
/// - `limits`: JSON text like `{"maxSteps":1000,"maxDepth":64}`, or `""`.
///
/// Resolves with JSON text: `{"ok":true,"value":…,"variables":…,"steps":…}`,
/// or `{"ok":false,"error":{…}}` for any CodyScript error (including a
/// program that isn't JSON). Rejects only when the arguments themselves are
/// wrong: variables or limits that aren't JSON, or an executor that isn't a
/// function.
#[wasm_bindgen]
pub async fn run(
    program: String,
    variables: String,
    executors: JsValue,
    limits: String,
) -> Result<String, JsError> {
    let variables: Variables = parse_or_default(&variables, "variables")?;
    let limits: Limits = parse_or_default(&limits, "limits")?;
    let engine = engine(&executors, limits)?;
    let outcome = match engine.compile_json(&program) {
        Ok(compiled) => compiled.run(RunOptions::from(variables)).await,
        Err(error) => Err(error),
    };
    outcome_json(&outcome).map_err(|e| JsError::new(&e.to_string()))
}

fn parse_or_default<T: DeserializeOwned + Default>(json: &str, what: &str) -> Result<T, JsError> {
    if json.trim().is_empty() {
        return Ok(T::default());
    }
    codyscript_json::from_str(json).map_err(|e| JsError::new(&format!("{what}: {e}")))
}

/// An engine whose executors are the JavaScript functions in `executors`.
fn engine(executors: &JsValue, limits: Limits) -> Result<Engine, JsError> {
    let mut builder = Engine::builder().limits(limits);
    if executors.is_null() || executors.is_undefined() {
        return Ok(builder.build());
    }
    let executors: &Object = executors
        .dyn_ref()
        .ok_or_else(|| JsError::new("executors: expected an object of functions"))?;
    for entry in Object::entries(executors).iter() {
        let entry: Array = entry.unchecked_into();
        let id = entry.get(0).as_string().unwrap_or_default();
        let function: Function = entry
            .get(1)
            .dyn_into()
            .map_err(|_| JsError::new(&format!("executors: \"{id}\" isn't a function")))?;
        builder = builder.custom_executor(id, JsExecutor(SendWrapper::new(function)));
    }
    Ok(builder.build())
}

// ── Outcomes, as JSON ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct Ran<'a> {
    ok: bool,
    value: &'a Value,
    variables: &'a Variables,
    steps: u64,
}

#[derive(Serialize)]
struct Failed<'a> {
    ok: bool,
    error: &'a Error,
}

fn outcome_json(outcome: &Result<RunResult, Error>) -> codyscript_json::serde_json::Result<String> {
    match outcome {
        Ok(result) => codyscript_json::to_string(&Ran {
            ok: true,
            value: &result.value,
            variables: &result.variables,
            steps: result.steps,
        }),
        Err(error) => codyscript_json::to_string(&Failed { ok: false, error }),
    }
}

// ── JavaScript executors ────────────────────────────────────────────────────

/// A JavaScript function as an [`Executor`]. JavaScript values aren't
/// `Send`; `SendWrapper` makes them so by checking, at runtime, that they
/// never leave the thread that made them — which in browser WebAssembly,
/// with its one thread, always holds.
struct JsExecutor(SendWrapper<Function>);

impl Executor for JsExecutor {
    fn call(&self, call: ExecutorCall) -> BoxFuture<'_, ExecutorResult> {
        Box::pin(SendWrapper::new(call_js(&self.0, call)))
    }
}

/// What a JavaScript executor receives: `{ op, arg?, input }`.
#[derive(Serialize)]
struct Call<'a> {
    op: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    arg: Option<&'a Value>,
    input: &'a Variables,
}

async fn call_js(function: &Function, call: ExecutorCall) -> ExecutorResult {
    let call = codyscript_json::to_string(&Call {
        op: &call.op,
        arg: call.arg.as_ref(),
        input: &call.input,
    })?;
    let argument = JSON::parse(&call).map_err(describe)?;
    let mut result = function
        .call1(&JsValue::NULL, &argument)
        .map_err(describe)?;
    if let Some(promise) = result.dyn_ref::<Promise>() {
        result = JsFuture::from(promise.clone()).await.map_err(describe)?;
    }
    value_of(&result)
}

/// A JavaScript value as a CodyScript value, through JSON: `undefined` (and
/// anything else JSON can't hold, like a function) is `null`.
fn value_of(result: &JsValue) -> ExecutorResult {
    if result.is_undefined() {
        return Ok(Value::Null);
    }
    let json = JsValue::from(JSON::stringify(result).map_err(describe)?);
    match json.as_string() {
        Some(json) => Ok(codyscript_json::from_str(&json)?),
        None => Ok(Value::Null),
    }
}

/// What JavaScript threw, as an error message: an `Error`'s message, a
/// string as itself, anything else as JSON.
fn describe(thrown: JsValue) -> BoxError {
    if let Some(error) = thrown.dyn_ref::<js_sys::Error>() {
        return String::from(error.message()).into();
    }
    if let Some(text) = thrown.as_string() {
        return text.into();
    }
    JSON::stringify(&thrown)
        .ok()
        .and_then(|json| JsValue::from(json).as_string())
        .unwrap_or_else(|| "a non-JSON value".into())
        .into()
}
