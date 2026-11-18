//! CodyScript in WebAssembly: JSON programs run from JavaScript, calling
//! external functions written in JavaScript.
//!
//! ```js
//! import init, { compile, run, debug } from "./pkg/codyscript_wasm.js";
//! await init();
//! const executors = {
//!   text: { trim: ({ input }) => String(input.value ?? "").trim() },
//!   slow: { echo: async ({ arg }) => { await new Promise((r) => setTimeout(r, 10)); return arg; } },
//! };
//! // Once: compile (throws a CodyScriptError if the program is invalid)…
//! const program = compile(programJson, executors, '{"max_steps":1000}');
//! // …then run as often as needed.
//! const outcome = JSON.parse(await program.run('{"Name":" Ada "}'));
//! // { ok: true, value, variables, steps }  or  { ok: false, error: { code, message, path, tag?, issues? } }
//!
//! // Or compile and run in one go, for a program run only once:
//! const once = JSON.parse(await run(programJson, '{"Name":" Ada "}', executors, ""));
//!
//! // Debugging, with a run configuration: pause at the step tagged "fee",
//! // then step over to the end.
//! const config = '{"variables":{"amount":200},"breakpoints":[{"tag":"fee"}]}';
//! const debugged = JSON.parse(await debug(programJson, config, executors, "", async (pause) => "step_over"));
//! ```
//!
//! Values cross the boundary as JSON text, so this exercises the whole stack
//! a host would use: `codyscript-json` parsing and printing, the core engine,
//! and async executors backed by JavaScript promises.

use std::rc::Rc;

use codyscript_core::debug::{Action, Breakpoint, Debugger, Pause, Step};
use codyscript_core::{
    BoxError, BoxFuture, CompiledProgram, Engine, Error, Executor, ExecutorCall, ExecutorFuture,
    ExecutorResult, Limits, RunConfig, RunOptions, RunResult, Value, Variables,
};
use codyscript_json::EngineJsonExt;
use js_sys::{Array, Function, JSON, Object, Promise};
use send_wrapper::SendWrapper;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, future_to_promise};

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

/// Compiles a program once, to run any number of times with `program.run`:
/// for a program that runs more than once, this saves parsing and compiling
/// it again each time, which costs several times what a short run does.
///
/// - `program`: JSON text.
/// - `executors` and `limits`: as for `run`.
///
/// Throws a `CodyScriptError` (an `Error` with `code`, `path`, and `tag` and
/// `issues` when there are any) if the program is invalid or isn't JSON, and
/// an `Error` if the arguments themselves are wrong: limits that aren't
/// JSON, an executor that isn't an object, or a function that isn't one.
#[wasm_bindgen]
pub fn compile(program: &str, executors: JsValue, limits: &str) -> Result<Program, JsValue> {
    match compile_program(program, &executors, limits)? {
        Ok(compiled) => Ok(Program {
            compiled: Rc::new(compiled),
        }),
        Err(error) => Err(thrown(&error)),
    }
}

/// A compiled program, with its executors and limits: made by
/// `compile`. Runs don't share state, and may overlap.
#[wasm_bindgen]
pub struct Program {
    /// Shared with the runs in flight, so freeing the program can't end them.
    compiled: Rc<CompiledProgram>,
}

#[wasm_bindgen]
impl Program {
    /// Runs the program with `variables` (JSON text of an object, or `""`
    /// for none). Resolves and rejects as the top-level `run` does.
    #[wasm_bindgen(unchecked_return_type = "Promise<string>")]
    pub fn run(&self, variables: String) -> Promise {
        let compiled = Rc::clone(&self.compiled);
        future_to_promise(async move {
            let variables: Variables = parse_or_default(&variables, "variables")?;
            let outcome = compiled.run(RunOptions::from(variables)).await;
            let json = outcome_json(&outcome).map_err(|e| JsError::new(&e.to_string()))?;
            Ok(json.into())
        })
    }
}

/// Compiles and runs a program: for one that runs once. To run a program
/// more than once, `compile` it once instead.
///
/// - `program`: JSON text.
/// - `variables`: JSON text of an object, or `""` for none.
/// - `executors`: `{ id: { function: (call) => value, … }, … }`, or `null`:
///   each executor an object of the external functions it provides, which
///   programs declare in `ext` and call as `id.function`. A function
///   receives `{ op, arg?, input }` (`op` is its own name) and returns a JSON
///   value, or a promise of one; `undefined` is `null`, and throwing (or
///   rejecting) fails the run with `function_failed`.
/// - `limits`: JSON text like `{"max_steps":1000,"max_depth":64}`, or `""`.
///
/// Resolves with JSON text: `{"ok":true,"value":…,"variables":…,"steps":…}`,
/// or `{"ok":false,"error":{…}}` for any CodyScript error (including a
/// program that isn't JSON). Rejects only when the arguments themselves are
/// wrong: variables or limits that aren't JSON, an executor that isn't an
/// object, or a function that isn't one.
#[wasm_bindgen]
pub async fn run(
    program: String,
    variables: String,
    executors: JsValue,
    limits: String,
) -> Result<String, JsError> {
    let variables: Variables = parse_or_default(&variables, "variables")?;
    let compiled = compile_program(&program, &executors, &limits)?;
    let outcome = match compiled {
        Ok(compiled) => compiled.run(RunOptions::from(variables)).await,
        Err(error) => Err(error),
    };
    outcome_json(&outcome).map_err(|e| JsError::new(&e.to_string()))
}

/// Lists a program's steps: what a breakpoint can name.
///
/// Takes `program`, `executors` and `limits` as [`run`] does (a program is
/// compiled, so its `ext` is checked against the executors). Returns JSON
/// text: `{"ok":true,"steps":[{"path","kind","name"?,"tag"?,"depth"},…]}`
/// — in the order written, the body's and then each function's — or
/// `{"ok":false,"error":{…}}` for a program that doesn't compile.
#[wasm_bindgen]
pub fn steps(program: String, executors: JsValue, limits: String) -> Result<String, JsError> {
    #[derive(Serialize)]
    struct Listed<'a> {
        ok: bool,
        steps: Vec<Step<'a>>,
    }
    let json = match compile_program(&program, &executors, &limits)? {
        Ok(compiled) => codyscript_json::to_string(&Listed {
            ok: true,
            steps: compiled.steps(),
        }),
        Err(error) => codyscript_json::to_string(&Failed {
            ok: false,
            error: &error,
        }),
    };
    json.map_err(|e| JsError::new(&e.to_string()))
}

/// Compiles and runs a program under a debugger written in JavaScript.
///
/// Takes `program`, `executors` and `limits` as [`run`] does; `config`, a
/// run configuration as JSON text — `{ "variables"?: {…}, "breakpoints"?:
/// [{ "tag": … } or { "path": … }, …] }`, or `""` for none (see
/// [`runConfigSchema`](run_config_schema)); and `onPause`, a function the
/// run calls each time it pauses before a step: at a breakpoint (`reason`
/// `"breakpoint"`), or where stepping ends (`"step"`) — nowhere else. It
/// receives `{ reason, path, kind, name?, tag?, function?, depth, steps,
/// frames: [{ function?, call?, scopes }], breakpoints }` — the step, as
/// [`steps`] lists it, and more — and returns, or resolves to (taking as
/// long as it likes), how to go on: `"step_over"`, `"step_into"`,
/// `"continue"` or `"stop"`, or `{ action, breakpoints? }` to change the
/// breakpoints too.
///
/// Resolves as `run` does; a stopped run is an `aborted` error, and a
/// breakpoint that names no step an `invalid_config` one. Rejects as `run`
/// does, if `config` isn't a run configuration, or if `onPause` throws,
/// rejects, or returns anything else.
#[wasm_bindgen]
pub async fn debug(
    program: String,
    config: String,
    executors: JsValue,
    limits: String,
    #[wasm_bindgen(js_name = "onPause")] on_pause: Function,
) -> Result<String, JsError> {
    let RunConfig {
        variables,
        breakpoints,
    } = parse_or_default(&config, "config")?;
    let mut debugger = JsDebugger {
        on_pause: SendWrapper::new(on_pause),
        failure: None,
    };
    let outcome = match compile_program(&program, &executors, &limits)? {
        Ok(compiled) => {
            let options = RunOptions::from(variables).with_debugger(breakpoints, &mut debugger);
            compiled.run(options).await
        }
        Err(error) => Err(error),
    };
    if let Some(failure) = debugger.failure {
        return Err(JsError::new(&format!("onPause: {failure}")));
    }
    outcome_json(&outcome).map_err(|e| JsError::new(&e.to_string()))
}

/// Checks a run configuration against a program: that each breakpoint
/// names one of its steps.
///
/// Takes `program`, `executors` and `limits` as [`run`] does, and `config`
/// as [`debug`] does. Returns JSON text: `{"ok":true}`, or
/// `{"ok":false,"error":{…}}` — `invalid_config`, listing every breakpoint
/// that names no step, or the program's own error if it doesn't compile.
/// Throws if `config` isn't a run configuration.
#[wasm_bindgen]
pub fn check(
    program: String,
    config: String,
    executors: JsValue,
    limits: String,
) -> Result<String, JsError> {
    #[derive(Serialize)]
    struct Checked {
        ok: bool,
    }
    let config: RunConfig = parse_or_default(&config, "config")?;
    let checked = compile_program(&program, &executors, &limits)?.and_then(|c| c.check(&config));
    let json = match checked {
        Ok(()) => codyscript_json::to_string(&Checked { ok: true }),
        Err(error) => codyscript_json::to_string(&Failed {
            ok: false,
            error: &error,
        }),
    };
    json.map_err(|e| JsError::new(&e.to_string()))
}

/// The JSON Schema for run configurations, as JSON text.
#[wasm_bindgen(js_name = "runConfigSchema")]
pub fn run_config_schema() -> String {
    codyscript_json::RUN_CONFIG_SCHEMA.into()
}

/// `program` compiled by an engine with `executors` and `limits`: a
/// CodyScript error for the program, a JavaScript one for the arguments.
fn compile_program(
    program: &str,
    executors: &JsValue,
    limits: &str,
) -> Result<Result<CompiledProgram, Error>, JsError> {
    let limits: Limits = parse_or_default(limits, "limits")?;
    let engine = engine(executors, limits)?;
    Ok(engine.compile_json(program))
}

fn parse_or_default<T: DeserializeOwned + Default>(json: &str, what: &str) -> Result<T, JsError> {
    if json.trim().is_empty() {
        return Ok(T::default());
    }
    codyscript_json::from_str(json).map_err(|e| JsError::new(&format!("{what}: {e}")))
}

/// An engine whose executors are the objects of JavaScript functions in
/// `executors`.
fn engine(executors: &JsValue, limits: Limits) -> Result<Engine, JsError> {
    let mut builder = Engine::builder().limits(limits);
    if executors.is_null() || executors.is_undefined() {
        return Ok(builder.build());
    }
    let executors: &Object = executors
        .dyn_ref()
        .ok_or_else(|| JsError::new("executors: expected an object of executors"))?;
    for (id, functions) in entries(executors) {
        let functions: &Object = functions
            .dyn_ref()
            .filter(|_| !functions.is_function())
            .ok_or_else(|| {
                JsError::new(&format!("executors: \"{id}\" isn't an object of functions"))
            })?;
        let mut by_name = HashMap::new();
        for (name, function) in entries(functions) {
            let function: Function = function.dyn_into().map_err(|_| {
                JsError::new(&format!("executors: \"{id}.{name}\" isn't a function"))
            })?;
            by_name.insert(name, function);
        }
        let names: Vec<String> = by_name.keys().cloned().collect();
        builder = builder.custom_executor(id, names, JsExecutor(SendWrapper::new(by_name)));
    }
    Ok(builder.build())
}

/// An object's own enumerable entries, keyed by string.
fn entries(object: &Object) -> Vec<(String, JsValue)> {
    Object::entries(object)
        .iter()
        .map(|entry| {
            let entry: Array = entry.unchecked_into();
            (entry.get(0).as_string().unwrap_or_default(), entry.get(1))
        })
        .collect()
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

/// A CodyScript error as JavaScript throws one: an `Error` named
/// `CodyScriptError`, with the error's fields (`code`, `message`, `path`,
/// `tag`, `issues`) as properties.
fn thrown(error: &Error) -> JsValue {
    let thrown = js_sys::Error::new(error.message());
    thrown.set_name("CodyScriptError");
    if let Some(fields) = codyscript_json::to_string(error)
        .ok()
        .and_then(|json| JSON::parse(&json).ok())
    {
        Object::assign(&thrown, fields.unchecked_ref());
    }
    thrown.into()
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

// ── A JavaScript debugger ───────────────────────────────────────────────────

/// `onPause` as a [`Debugger`]. If it fails, the run stops, and `debug`
/// rejects with `failure`.
struct JsDebugger {
    on_pause: SendWrapper<Function>,
    failure: Option<BoxError>,
}

/// What `onPause` may return: an action, or `{ action, breakpoints? }`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Reply {
    Action(Action),
    With {
        action: Action,
        breakpoints: Option<Vec<Breakpoint>>,
    },
}

impl Reply {
    /// How to go on, and the new breakpoints, if any.
    fn into_parts(self) -> (Action, Option<Vec<Breakpoint>>) {
        match self {
            Reply::Action(action) => (action, None),
            Reply::With {
                action,
                breakpoints,
            } => (action, breakpoints),
        }
    }
}

impl Debugger for JsDebugger {
    fn pause<'a>(&'a mut self, mut pause: Pause<'a>) -> BoxFuture<'a, Action> {
        Box::pin(SendWrapper::new(async move {
            let (action, breakpoints) = match ask(&self.on_pause, &pause).await {
                Ok(reply) => reply.into_parts(),
                Err(failure) => {
                    self.failure = Some(failure);
                    return Action::Stop;
                }
            };
            if let Some(breakpoints) = breakpoints {
                *pause.breakpoints_mut() = breakpoints;
            }
            action
        }))
    }
}

/// Calls `onPause` with the pause, as a plain object, and reads its reply.
async fn ask(on_pause: &Function, pause: &Pause<'_>) -> Result<Reply, BoxError> {
    let pause = JSON::parse(&codyscript_json::to_string(pause)?).map_err(describe)?;
    let mut reply = on_pause.call1(&JsValue::NULL, &pause).map_err(describe)?;
    if let Some(promise) = reply.dyn_ref::<Promise>() {
        reply = JsFuture::from(promise.clone()).await.map_err(describe)?;
    }
    let json = JSON::stringify(&reply)
        .ok()
        .and_then(|json| JsValue::from(json).as_string())
        .unwrap_or_else(|| "undefined".into());
    codyscript_json::from_str(&json).map_err(|_| {
        let expected =
            r#""step_over", "step_into", "continue", "stop" or { action, breakpoints? }"#;
        format!("expected {expected}, got {json}").into()
    })
}

// ── JavaScript executors ────────────────────────────────────────────────────

/// An object of JavaScript functions as an [`Executor`], calling the one
/// named by the call's `op`. JavaScript values aren't `Send`; `SendWrapper`
/// makes them so by checking, at runtime, that they never leave the thread
/// that made them — which in browser WebAssembly, with its one thread,
/// always holds.
struct JsExecutor(SendWrapper<HashMap<String, Function>>);

impl Executor for JsExecutor {
    /// Calls the function at once: a value it returns is the result, and
    /// only a promise is awaited.
    fn call(&self, call: ExecutorCall) -> ExecutorFuture<'_> {
        // The engine only calls functions this executor was registered
        // with: its object's keys.
        let Some(function) = self.0.get(call.op()) else {
            return Err("no such function".into()).into();
        };
        match call_js(function, &call) {
            Ok(result) => match result.dyn_into::<Promise>() {
                Ok(promise) => ExecutorFuture::new(SendWrapper::new(async move {
                    value_of(&JsFuture::from(promise).await.map_err(describe)?)
                })),
                Err(result) => value_of(&result).into(),
            },
            Err(error) => Err(error).into(),
        }
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

/// Calls `function` with `call`: what it returns, maybe a promise.
fn call_js(function: &Function, call: &ExecutorCall) -> Result<JsValue, BoxError> {
    let call = codyscript_json::to_string(&Call {
        op: call.op(),
        arg: call.arg(),
        input: &call.input,
    })?;
    let argument = JSON::parse(&call).map_err(describe)?;
    function.call1(&JsValue::NULL, &argument).map_err(describe)
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
