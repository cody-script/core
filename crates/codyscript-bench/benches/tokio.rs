//! Async executors under Tokio: what each way of running costs, and what
//! concurrency buys. Compare with `benches/wasm.rs`, which runs the same
//! scenarios in single-threaded WebAssembly.
//!
//! `cargo bench -p codyscript-bench --bench tokio`

use std::sync::Arc;
use std::time::Duration;

use codyscript_bench as scenario;
use codyscript_core::{CompiledProgram, Engine};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use tokio::runtime::{Builder, Runtime};

/// Executor calls per run (`call_loop` with `n` = this).
const CALLS: u64 = 100;
/// Runs per batch, in the concurrency benchmarks.
const RUNS: u64 = 64;

fn current_thread() -> Runtime {
    Builder::new_current_thread().build().expect("a runtime")
}

/// One worker per core.
fn multi_thread() -> Runtime {
    Builder::new_multi_thread().build().expect("a runtime")
}

fn call_loop(engine: &Engine) -> Arc<CompiledProgram> {
    Arc::new(scenario::compile(engine, &scenario::call_loop()))
}

/// One run, `CALLS` executor calls: blocking, or on either runtime, with
/// sync, ready-async and yielding executors.
fn executor(c: &mut Criterion) {
    let sync = call_loop(&scenario::engine());
    let ready = call_loop(&scenario::engine_with_async_math());
    let yielding = call_loop(&scenario::yielding_engine());
    let (current, multi) = (current_thread(), multi_thread());
    let options = || scenario::options(scenario::n(CALLS));

    let mut group = c.benchmark_group("tokio/executor");
    group.throughput(Throughput::Elements(CALLS));
    group.bench_function("blocking/sync", |b| b.iter(|| sync.run_blocking(options())));
    group.bench_function("blocking/async_ready", |b| {
        b.iter(|| ready.run_blocking(options()))
    });
    for (runtime_name, runtime) in [("current_thread", &current), ("multi_thread", &multi)] {
        for (executor_name, program) in [
            ("sync", &sync),
            ("async_ready", &ready),
            ("yielding", &yielding),
        ] {
            group.bench_function(format!("{runtime_name}/{executor_name}"), |b| {
                b.to_async(runtime).iter(|| program.run(options()))
            });
        }
    }
    group.finish();
}

/// `RUNS` runs of one compiled program, each making `CALLS` calls: one after
/// another, interleaved on one thread, or spread over every core.
fn concurrency(c: &mut Criterion) {
    let (current, multi) = (current_thread(), multi_thread());
    let mut group = c.benchmark_group("tokio/concurrency");
    group.throughput(Throughput::Elements(RUNS));
    for (executor_name, engine) in [
        ("sync", scenario::engine()),
        ("yielding", scenario::yielding_engine()),
    ] {
        let program = call_loop(&engine);
        group.bench_function(format!("sequential/{executor_name}"), |b| {
            b.to_async(&current).iter(|| {
                let program = Arc::clone(&program);
                async move {
                    for _ in 0..RUNS {
                        program
                            .run(scenario::options(scenario::n(CALLS)))
                            .await
                            .unwrap();
                    }
                }
            })
        });
        for (runtime_name, runtime) in [("current_thread", &current), ("multi_thread", &multi)] {
            group.bench_function(format!("{runtime_name}/{executor_name}"), |b| {
                b.to_async(runtime).iter(|| spawn_all(Arc::clone(&program)))
            });
        }
    }
    group.finish();
}

/// Spawns `RUNS` runs as tasks and waits for them all.
async fn spawn_all(program: Arc<CompiledProgram>) {
    let tasks: Vec<_> = (0..RUNS)
        .map(|_| {
            let program = Arc::clone(&program);
            tokio::spawn(async move { program.run(scenario::options(scenario::n(CALLS))).await })
        })
        .collect();
    for task in tasks {
        task.await
            .expect("the task finished")
            .expect("the run succeeded");
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = executor, concurrency
}
criterion_main!(benches);
