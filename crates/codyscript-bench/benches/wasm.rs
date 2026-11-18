//! The same scenarios in single-threaded WebAssembly, run by
//! `wasm-bindgen-test-runner` in Node with wasm-bindgen-test's port of
//! Criterion. Names match `benches/speed.rs` and `benches/tokio.rs`, so the
//! reports compare line by line; the memory table matches
//! `benches/memory.rs`.
//!
//! `cargo bench -p codyscript-bench --target wasm32-unknown-unknown --bench wasm`

#![cfg(target_arch = "wasm32")]

use std::hint::black_box;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use codyscript_bench::alloc::{Counting, bytes, measure, report};
use codyscript_bench::{self as scenario, memory_scenarios};
use codyscript_core::CompiledProgram;
use codyscript_json::EngineJsonExt;
use js_sys::{Function, Object, Reflect};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{Criterion, wasm_bindgen_bench};

#[global_allocator]
static ALLOC: Counting = Counting;

/// As in `benches/tokio.rs`.
const CALLS: u64 = 100;
const RUNS: u64 = 64;

/// The same timing budget as the native benchmarks.
fn configure(c: &mut Criterion) {
    *c = std::mem::take(c)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
}

#[wasm_bindgen_bench]
fn compile(c: &mut Criterion) {
    configure(c);
    let engine = scenario::engine();
    for (name, program) in [
        ("compile/readme", scenario::readme_rule()),
        ("compile/fee", scenario::fee_rule()),
        ("compile/deep", scenario::deep_program()),
        ("compile/wide/100", scenario::wide_program(100)),
    ] {
        c.bench_function(name, |b| {
            b.iter(|| scenario::compile(&engine, black_box(&program)))
        });
    }
}

#[wasm_bindgen_bench]
fn run(c: &mut Criterion) {
    configure(c);
    let engine = scenario::engine();
    let readme = scenario::compile(&engine, &scenario::readme_rule());
    c.bench_function("run/readme", |b| {
        b.iter(|| readme.run_blocking(scenario::options(scenario::readme_variables())))
    });
    let fee = scenario::compile(&engine, &scenario::fee_rule());
    c.bench_function("run/fee", |b| {
        b.iter(|| fee.run_blocking(scenario::options(scenario::fee_variables())))
    });
    let sum = scenario::compile(&engine, &scenario::sum_loop());
    c.bench_function("run/sum_loop/1000", |b| {
        b.iter(|| sum.run_blocking(scenario::options(scenario::n(1_000))))
    });
    let calls = scenario::compile(&engine, &scenario::call_loop());
    c.bench_function("run/call_loop/1000", |b| {
        b.iter(|| calls.run_blocking(scenario::options(scenario::n(1_000))))
    });
    let functions = scenario::compile(&engine, &scenario::function_loop());
    c.bench_function("run/function_loop/1000", |b| {
        b.iter(|| functions.run_blocking(scenario::options(scenario::n(1_000))))
    });
    let debugged = || {
        let options = scenario::options(scenario::n(1_000))
            .with_debugger(scenario::at_the_end(&sum), scenario::passing_debugger());
        sum.run_blocking(options)
    };
    // A run that fails (a breakpoint naming no step, say) would be quick.
    assert_eq!(debugged().unwrap().steps, scenario::sum_loop_steps(1_000));
    c.bench_function("debug/sum_loop/1000", |b| b.iter(debugged));
}

#[wasm_bindgen_bench]
fn json(c: &mut Criterion) {
    configure(c);
    let engine = scenario::engine();
    let rule_json = scenario::readme_rule_json();
    c.bench_function("json/parse", |b| {
        b.iter(|| codyscript_json::from_str::<codyscript_core::Value>(black_box(&rule_json)))
    });
    c.bench_function("json/end_to_end", |b| {
        b.iter(|| scenario::json_end_to_end(&engine, &rule_json, r#"{"Name":"  Ada Lovelace  "}"#))
    });
    let rule = engine.compile_json(&rule_json).expect("the rule compiles");
    c.bench_function("json/compiled_once", |b| {
        b.iter(|| scenario::json_row(&rule, r#"{"Name":"  Ada Lovelace  "}"#))
    });
}

/// The README rule through the JavaScript bindings, with its `text`
/// executor in JavaScript: compiled and run for every row (`run`), or
/// compiled once and run per row (`compile`, then `program.run`).
#[wasm_bindgen_bench]
async fn bindings(c: &mut Criterion) {
    configure(c);
    let program = scenario::readme_rule_json();
    let row = r#"{"Name":"  Ada Lovelace  "}"#;
    let executors = text_in_javascript();
    let (p, e) = (program.clone(), executors.clone());
    c.bench_async_function("wasm/readme/run", move |b| {
        let (program, executors) = (p.clone(), e.clone());
        Box::pin(b.iter_future(move || {
            codyscript_wasm::run(
                program.clone(),
                row.into(),
                executors.clone(),
                String::new(),
            )
        }))
    })
    .await;
    let compiled = Rc::new(codyscript_wasm::compile(&program, executors, "").expect("compiles"));
    c.bench_async_function("wasm/readme/compiled_once", move |b| {
        let compiled = Rc::clone(&compiled);
        Box::pin(b.iter_future(move || JsFuture::from(compiled.run(row.into()))))
    })
    .await;
}

/// One run, `CALLS` executor calls — blocking, or awaited on the JavaScript
/// event loop (the counterpart of Tokio's current-thread runtime) — and the
/// same through the JavaScript bindings, with JavaScript executors.
#[wasm_bindgen_bench]
async fn executor(c: &mut Criterion) {
    configure(c);
    let sync = call_loop(&scenario::engine());
    let ready = call_loop(&scenario::engine_with_async_math());
    let yielding = call_loop(&scenario::yielding_engine());
    let options = || scenario::options(scenario::n(CALLS));

    c.bench_function("tokio/executor/blocking/sync", |b| {
        b.iter(|| sync.run_blocking(options()))
    });
    c.bench_function("tokio/executor/blocking/async_ready", |b| {
        b.iter(|| ready.run_blocking(options()))
    });
    for (name, program) in [
        ("sync", &sync),
        ("async_ready", &ready),
        ("yielding", &yielding),
    ] {
        let program = Arc::clone(program);
        c.bench_async_function(&format!("tokio/executor/event_loop/{name}"), move |b| {
            let program = Arc::clone(&program);
            Box::pin(b.iter_future(move || {
                let program = Arc::clone(&program);
                async move { program.run(scenario::options(scenario::n(CALLS))).await }
            }))
        })
        .await;
    }

    // Through the bindings: JSON in and out, executors in JavaScript.
    let program = codyscript_json::to_string(&scenario::call_loop()).unwrap();
    let variables = format!(r#"{{"n":{CALLS}}}"#);
    for (name, body) in [
        ("sync", "return call.input.a + call.input.b;"),
        (
            "promise",
            "return Promise.resolve(call.input.a + call.input.b);",
        ),
    ] {
        let executors = math_in_javascript(body);
        let (program, variables) = (program.clone(), variables.clone());
        c.bench_async_function(&format!("wasm/js_executor/{name}"), move |b| {
            let (program, variables, executors) =
                (program.clone(), variables.clone(), executors.clone());
            Box::pin(b.iter_future(move || {
                codyscript_wasm::run(
                    program.clone(),
                    variables.clone(),
                    executors.clone(),
                    String::new(),
                )
            }))
        })
        .await;
    }
}

/// `RUNS` runs of one compiled program: one after another, or interleaved
/// on the event loop — WebAssembly's only kind of concurrency.
#[wasm_bindgen_bench]
async fn concurrency(c: &mut Criterion) {
    configure(c);
    for (name, engine) in [
        ("sync", scenario::engine()),
        ("yielding", scenario::yielding_engine()),
    ] {
        let program = call_loop(&engine);
        let sequential = Arc::clone(&program);
        c.bench_async_function(&format!("tokio/concurrency/sequential/{name}"), move |b| {
            let program = Arc::clone(&sequential);
            Box::pin(b.iter_future(move || {
                let program = Arc::clone(&program);
                async move {
                    for _ in 0..RUNS {
                        program
                            .run(scenario::options(scenario::n(CALLS)))
                            .await
                            .unwrap();
                    }
                }
            }))
        })
        .await;
        c.bench_async_function(&format!("tokio/concurrency/event_loop/{name}"), move |b| {
            let program = Arc::clone(&program);
            Box::pin(b.iter_future(move || {
                let program = Arc::clone(&program);
                async move {
                    let runs =
                        (0..RUNS).map(|_| program.run(scenario::options(scenario::n(CALLS))));
                    scenario::join_all(runs.collect()).await
                }
            }))
        })
        .await;
    }
}

/// Not timed: prints the memory table, as `benches/memory.rs` does natively.
#[wasm_bindgen_bench]
fn memory(_: &mut Criterion) {
    let mut rows = Vec::new();
    for (name, scenario) in memory_scenarios() {
        drop(scenario());
        let (kept, usage) = measure(&scenario); // borrowed: dropping it isn't part of the work
        drop(kept);
        rows.push((name, usage));
    }
    let pages = core::arch::wasm32::memory_size(0);
    // `println!` has no stdout to go to here; the console does.
    wasm_bindgen_test::console_log!(
        "\nWebAssembly memory (counted by the global allocator):\n\n{}\nLinear memory: {} ({pages} pages of 64 KiB)\n",
        report(&rows),
        bytes((pages * 65536) as isize)
    );
}

fn call_loop(engine: &codyscript_core::Engine) -> Arc<CompiledProgram> {
    Arc::new(scenario::compile(engine, &scenario::call_loop()))
}

/// `{ text: { trim, empty } }`: the README rule's functions.
fn text_in_javascript() -> JsValue {
    let text = Object::new();
    let trim = "return String(call.input.value ?? '').trim();";
    let empty = "return String(call.input.value ?? '').length === 0;";
    for (name, body) in [("trim", trim), ("empty", empty)] {
        Reflect::set(&text, &name.into(), &Function::new_with_args("call", body)).unwrap();
    }
    let executors = Object::new();
    Reflect::set(&executors, &"text".into(), &text).unwrap();
    executors.into()
}

/// `{ math: (call) => { <body> } }`.
fn math_in_javascript(body: &str) -> JsValue {
    let math = Object::new();
    Reflect::set(&math, &"add".into(), &Function::new_with_args("call", body)).unwrap();
    let executors = Object::new();
    Reflect::set(&executors, &"math".into(), &math).unwrap();
    executors.into()
}
