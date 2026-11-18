//! The scenarios CodyScript's benchmarks measure, shared by all of them —
//! native (`benches/speed.rs`, `benches/tokio.rs`, `benches/memory.rs`) and
//! WebAssembly (`benches/wasm.rs`) — so their numbers compare like for like.
//!
//! Programs are built with `value!`; variables and engines come ready to run.

pub mod alloc;

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use codyscript_core::debug::{self, Action, Breakpoint, Debugger};
use codyscript_core::{
    CompiledProgram, Engine, ExecutorCall, ExecutorResult, RunOptions, Value, Variables, value,
};
use codyscript_json::EngineJsonExt;

// ── Programs ────────────────────────────────────────────────────────────────

/// The README's rule: trims and checks a `Name` with the `text` executor's
/// functions. Two external calls, 8 steps.
pub fn readme_rule() -> Value {
    value!({ "ext": { "text": ["trim", "empty"] }, "do": [
        { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
        { "if": { "call": { "text.empty": { "value": { "ref": "name" } } } },
          "do": [{ "ret": { "val": { "error": "Name is required" } } }], "tag": "name-required" },
        { "ret": { "ref": "name" } }
    ] })
}

/// The README rule as JSON text, as it would be stored.
pub fn readme_rule_json() -> String {
    codyscript_json::to_string(&readme_rule()).expect("JSON")
}

/// A capped fee: expressions only, no executors. 14 steps.
pub fn fee_rule() -> Value {
    value!({ "do": [
        { "set": { "fee": { "x": "amount * 0.03" } } },
        { "if": { "x": "fee > 300" }, "do": [{ "set": { "fee": 300 } }] },
        { "ret": { "ref": "fee" } }
    ] })
}

/// Sums 1..`n` in a loop of expressions: interpreter overhead per step.
pub fn sum_loop() -> Value {
    value!({ "do": [
        { "set": { "i": 0 } }, { "set": { "sum": 0 } },
        { "for": { "x": "i < n" }, "do": [
            { "set": { "i": { "x": "i + 1" } } },
            { "set": { "sum": { "x": "sum + i" } } }
        ] },
        { "ret": { "ref": "sum" } }
    ] })
}

/// Steps `sum_loop` takes for `n`: 11, plus 14 per pass.
pub fn sum_loop_steps(n: u64) -> u64 {
    // Two sets (2 each), the loop (1), its last, failing test (4) and the ret
    // (2); each pass, a test (4) and two sets (1 + 4 each).
    11 + 14 * n
}

/// Counts to `n` by calling the external function `math.add` `n` times:
/// the cost of an executor call.
pub fn call_loop() -> Value {
    value!({ "ext": { "math": ["add"] }, "do": [
        { "set": { "i": 0 } },
        { "for": { "x": "i < n" }, "do": [
            { "set": { "i": { "call": { "math.add": { "a": { "ref": "i" }, "b": 1 } } } } }
        ] },
        { "ret": { "ref": "i" } }
    ] })
}

/// Counts to `n` by calling the CodyScript function `next` `n` times: the
/// cost of a call of a program's own function (a fresh scope each time).
pub fn function_loop() -> Value {
    value!({
        "fn": { "next": { "x": "i + 1" } },
        "do": [
            { "set": { "i": 0 } },
            { "for": { "x": "i < n" }, "do": [
                { "set": { "i": { "call": { "next": { "i": { "ref": "i" } } } } } }
            ] },
            { "ret": { "ref": "i" } }
        ]
    })
}

/// `width` independent instructions, each with its own expression: compile
/// cost per node.
pub fn wide_program(width: usize) -> Value {
    let body: Vec<Value> = (0..width)
        .map(|i| value!({ "set": { (format!("v{i}")): { "x": (format!("a * {i} + b")) } } }))
        .collect();
    value!({ "do": body })
}

/// The deepest program the default `max_depth` (128) allows: 42 nested
/// blocks, three nesting levels each.
pub fn deep_program() -> Value {
    let mut program = value!(1);
    for _ in 0..42 {
        program = value!({ "do": [{ "ret": program }] });
    }
    program
}

/// Reads `last` `n` times in a loop — from a scope holding however many
/// variables it's given: the cost of a lookup against the size of the scope.
pub fn lookup_loop() -> Value {
    value!({ "do": [
        { "set": { "i": 0 } },
        { "for": { "x": "i < n" }, "do": [{ "set": { "i": { "x": "i + last" } } }] },
        { "ret": { "ref": "i" } }
    ] })
}

// ── Variables ───────────────────────────────────────────────────────────────

/// The README rule's input.
pub fn readme_variables() -> Variables {
    vars(value!({ "Name": "  Ada Lovelace  " }))
}

/// `{ "n": n }`.
pub fn n(n: u64) -> Variables {
    vars(value!({ "n": n }))
}

/// `{ "amount": 20000 }`.
pub fn fee_variables() -> Variables {
    vars(value!({ "amount": 20000 }))
}

/// `count` variables, `v0`…, then `last` (1) and `n`.
pub fn many_variables(count: usize, n: u64) -> Variables {
    let mut variables: Variables = (0..count).map(|i| (format!("v{i}"), i)).collect();
    variables.insert("last".into(), Value::from(1));
    variables.insert("n".into(), Value::from(n));
    variables
}

fn vars(value: Value) -> Variables {
    value.into_object().expect("an object")
}

/// Run options with these variables.
pub fn options<'d>(variables: Variables) -> RunOptions<'d> {
    RunOptions::from(variables)
}

// ── Executors and engines ───────────────────────────────────────────────────

/// `text.trim` and `text.empty`.
pub fn text(call: ExecutorCall) -> ExecutorResult {
    let value = call
        .input
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or("");
    match call.op() {
        "trim" => Ok(value.trim().into()),
        "empty" => Ok(value.is_empty().into()),
        op => Err(format!("text has no {op:?}").into()),
    }
}

/// `math.add` of integers.
pub fn math(call: ExecutorCall) -> ExecutorResult {
    let arg = |name: &str| call.input.get(name).and_then(Value::as_i64).unwrap_or(0);
    Ok(Value::from(arg("a") + arg("b")))
}

/// An engine whose executors are synchronous, with room for long loops.
pub fn engine() -> Engine {
    Engine::builder()
        .executor("text", ["trim", "empty"], text)
        .executor("math", ["add"], math)
        .max_steps(u64::MAX)
        .build()
}

/// An engine whose `math` executor is `async`, but ready at once: the cost
/// of the async path itself, with no runtime involved.
pub fn engine_with_async_math() -> Engine {
    Engine::builder()
        .async_executor("math", ["add"], |call| async move { math(call) })
        .max_steps(u64::MAX)
        .build()
}

/// An engine whose `math` executor yields once on every call before
/// answering, as one awaiting I/O would: each call suspends the run and goes
/// back through the runtime's scheduler — Tokio's natively, the JavaScript
/// event loop in WebAssembly. The same code on both, for a fair comparison.
pub fn yielding_engine() -> Engine {
    Engine::builder()
        .async_executor("math", ["add"], |call| async move {
            yield_now().await;
            math(call)
        })
        .max_steps(u64::MAX)
        .build()
}

/// A breakpoint on `program`'s last step, so a debugged run checks one
/// before every step but pauses only once, at the very end: what debugging
/// itself costs, with no one to wait for.
pub fn at_the_end(program: &CompiledProgram) -> [Breakpoint; 1] {
    let steps = program.steps();
    let last = steps.last().expect("the scenario has steps");
    [Breakpoint::path(last.path)]
}

/// A debugger that goes straight on.
pub fn passing_debugger() -> impl Debugger {
    debug::from_fn(|_| Action::Continue)
}

/// Returns `Pending` once, waking itself: the task is scheduled again.
pub fn yield_now() -> impl Future<Output = ()> + Send {
    let mut yielded = false;
    std::future::poll_fn(move |cx: &mut Context<'_>| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
}

/// Polls every future in turn until all are done, on the current task: runs
/// interleaved on one thread, with no runtime to spawn on.
pub async fn join_all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut pending: Vec<Option<Pin<Box<F>>>> =
        futures.into_iter().map(|f| Some(Box::pin(f))).collect();
    let mut outputs: Vec<Option<F::Output>> = pending.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut done = true;
        for (slot, output) in pending.iter_mut().zip(outputs.iter_mut()) {
            if let Some(future) = slot {
                match future.as_mut().poll(cx) {
                    Poll::Ready(value) => {
                        *output = Some(value);
                        *slot = None;
                    }
                    Poll::Pending => done = false,
                }
            }
        }
        if done { Poll::Ready(()) } else { Poll::Pending }
    })
    .await;
    outputs
        .into_iter()
        .map(|o| o.expect("every future finished"))
        .collect()
}

/// `program`, compiled by `engine`.
pub fn compile(engine: &Engine, program: &Value) -> CompiledProgram {
    engine.compile(program).expect("the scenario compiles")
}

/// Parses, compiles, runs and prints the README rule, as a JSON-in,
/// JSON-out host that compiles the rule for every row would.
pub fn json_end_to_end(engine: &Engine, rule_json: &str, row_json: &str) -> String {
    let rule = engine.compile_json(rule_json).expect("the rule compiles");
    json_row(&rule, row_json)
}

/// Parses a row, runs a compiled rule on it and prints the result: what a
/// row costs a host that compiles its rule once.
pub fn json_row(rule: &CompiledProgram, row_json: &str) -> String {
    let row: Variables = codyscript_json::from_str(row_json).expect("JSON");
    let result = rule
        .run_blocking(RunOptions::from(row))
        .expect("the rule runs");
    codyscript_json::to_string(&result).expect("JSON")
}

// ── Memory ──────────────────────────────────────────────────────────────────

/// A scenario for the memory report: what it does, and a function doing it
/// that returns whatever it keeps (so its size counts as retained).
pub type MemoryScenario = (&'static str, Box<dyn Fn() -> Box<dyn std::any::Any>>);

/// The memory scenarios both native and WebAssembly report on.
pub fn memory_scenarios() -> Vec<MemoryScenario> {
    fn keep<T: 'static>(value: T) -> Box<dyn std::any::Any> {
        Box::new(value)
    }
    let engine = engine();
    let readme = compile(&engine, &readme_rule());
    let sum = compile(&engine, &sum_loop());
    let calls = compile(&engine, &call_loop());
    let calls_async = compile(&engine_with_async_math(), &call_loop());
    let rule_json = readme_rule_json();
    let readme_json = engine.compile_json(&rule_json).expect("the rule compiles");
    let (e1, e2, e3) = (engine.clone(), engine.clone(), engine.clone());
    // Built here, not in the scenarios: compiling borrows a program, and
    // building one isn't the work being measured.
    let (rule, wide, deep) = (readme_rule(), wide_program(100), deep_program());
    vec![
        (
            "parse the README rule from JSON",
            Box::new(move || keep(codyscript_json::from_str::<Value>(&rule_json).expect("JSON"))),
        ),
        (
            "compile the README rule",
            Box::new(move || keep(compile(&e1, &rule))),
        ),
        (
            "compile a wide program (100 instructions)",
            Box::new(move || keep(compile(&e2, &wide))),
        ),
        (
            "compile the deepest default program",
            Box::new(move || keep(compile(&e3, &deep))),
        ),
        (
            "run the README rule",
            Box::new(move || keep(readme.run_blocking(options(readme_variables())))),
        ),
        (
            "run the sum loop, n = 1000",
            Box::new(move || keep(sum.run_blocking(options(n(1000))))),
        ),
        (
            "run the call loop, n = 1000, sync executor",
            Box::new(move || keep(calls.run_blocking(options(n(1000))))),
        ),
        (
            "run the call loop, n = 1000, async executor",
            Box::new(move || keep(calls_async.run_blocking(options(n(1000))))),
        ),
        (
            "JSON in, run, JSON out (README rule)",
            Box::new(move || {
                keep(json_end_to_end(
                    &engine,
                    &readme_rule_json(),
                    r#"{"Name":"  Ada  "}"#,
                ))
            }),
        ),
        (
            "JSON in, run, JSON out (README rule, compiled once)",
            Box::new(move || keep(json_row(&readme_json, r#"{"Name":"  Ada  "}"#))),
        ),
    ]
}
