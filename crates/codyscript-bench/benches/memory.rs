//! What CodyScript allocates, natively: allocations, bytes, peak and
//! retained memory per scenario, counted by a global allocator. Printed as a
//! Markdown table. Compare with the table `benches/wasm.rs` prints.
//!
//! `cargo bench -p codyscript-bench --bench memory`

use std::sync::Arc;

use codyscript_bench::alloc::{Counting, Usage, measure, report};
use codyscript_bench::{self as scenario, memory_scenarios};

#[global_allocator]
static ALLOC: Counting = Counting;

/// Runs per batch in the Tokio rows, as in `benches/tokio.rs`.
const RUNS: u64 = 64;

fn main() {
    let mut rows: Vec<(&str, Usage)> = Vec::new();
    for (name, scenario) in memory_scenarios() {
        drop(scenario()); // warm up: lazily allocated statics don't count
        let (kept, usage) = measure(&scenario); // borrowed: dropping it isn't part of the work
        drop(kept);
        rows.push((name, usage));
    }
    rows.extend(tokio_rows());
    println!(
        "\nNative memory (counted by the global allocator):\n\n{}",
        report(&rows)
    );
}

/// `RUNS` runs of the call loop (n = 100) on a multi-threaded runtime: one
/// after another, then all at once. Peak memory grows with the runs in
/// flight.
fn tokio_rows() -> Vec<(&'static str, Usage)> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .build()
        .expect("a runtime");
    let program = Arc::new(scenario::compile(
        &scenario::engine(),
        &scenario::call_loop(),
    ));
    let sequential = |program: Arc<codyscript_core::CompiledProgram>| async move {
        for _ in 0..RUNS {
            program
                .run(scenario::options(scenario::n(100)))
                .await
                .unwrap();
        }
    };
    let concurrent = |program: Arc<codyscript_core::CompiledProgram>| async move {
        let tasks: Vec<_> = (0..RUNS)
            .map(|_| {
                let program = Arc::clone(&program);
                tokio::spawn(async move { program.run(scenario::options(scenario::n(100))).await })
            })
            .collect();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
    };
    runtime.block_on(concurrent(Arc::clone(&program))); // warm up the workers
    let ((), one_by_one) = measure(|| runtime.block_on(sequential(Arc::clone(&program))));
    let ((), all_at_once) = measure(|| runtime.block_on(concurrent(Arc::clone(&program))));
    vec![
        (
            "Tokio: 64 runs of the call loop (n = 100), one by one",
            one_by_one,
        ),
        (
            "Tokio: 64 runs of the call loop (n = 100), concurrently",
            all_at_once,
        ),
    ]
}
