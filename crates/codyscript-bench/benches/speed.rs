//! How fast CodyScript compiles and runs, natively, on one thread.
//!
//! `cargo bench -p codyscript-bench --bench speed`

use std::hint::black_box;
use std::time::Duration;

use codyscript_bench as scenario;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

/// Compiling: validation, expression parsing and building the program tree.
fn compile(c: &mut Criterion) {
    let engine = scenario::engine();
    let mut group = c.benchmark_group("compile");
    for (name, program) in [
        ("readme", scenario::readme_rule()),
        ("fee", scenario::fee_rule()),
        ("deep", scenario::deep_program()),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| scenario::compile(&engine, black_box(&program)))
        });
    }
    for width in [10, 100, 1000] {
        let program = scenario::wide_program(width);
        group.throughput(Throughput::Elements(width as u64));
        group.bench_with_input(BenchmarkId::new("wide", width), &program, |b, program| {
            b.iter(|| scenario::compile(&engine, black_box(program)))
        });
    }
    group.finish();
}

/// Running compiled programs, with synchronous executors.
fn run(c: &mut Criterion) {
    let engine = scenario::engine();
    let mut group = c.benchmark_group("run");

    let readme = scenario::compile(&engine, &scenario::readme_rule());
    group.bench_function("readme", |b| {
        b.iter(|| readme.run_blocking(scenario::options(scenario::readme_variables())))
    });
    let fee = scenario::compile(&engine, &scenario::fee_rule());
    group.bench_function("fee", |b| {
        b.iter(|| fee.run_blocking(scenario::options(scenario::fee_variables())))
    });
    let deep = scenario::compile(&engine, &scenario::deep_program());
    group.bench_function("deep", |b| {
        b.iter(|| deep.run_blocking(scenario::options(Default::default())))
    });

    // Steps per second, as the loop grows: should stay flat.
    let sum = scenario::compile(&engine, &scenario::sum_loop());
    for n in [10, 100, 1_000, 10_000] {
        let steps = sum
            .run_blocking(scenario::options(scenario::n(n)))
            .unwrap()
            .steps;
        assert_eq!(steps, scenario::sum_loop_steps(n));
        group.throughput(Throughput::Elements(steps));
        group.bench_with_input(BenchmarkId::new("sum_loop", n), &n, |b, &n| {
            b.iter(|| sum.run_blocking(scenario::options(scenario::n(n))))
        });
    }

    // Executor calls per second.
    let calls = scenario::compile(&engine, &scenario::call_loop());
    group.throughput(Throughput::Elements(1_000));
    group.bench_function("call_loop/1000", |b| {
        b.iter(|| calls.run_blocking(scenario::options(scenario::n(1_000))))
    });

    // CodyScript function calls per second.
    let functions = scenario::compile(&engine, &scenario::function_loop());
    group.throughput(Throughput::Elements(1_000));
    group.bench_function("function_loop/1000", |b| {
        b.iter(|| functions.run_blocking(scenario::options(scenario::n(1_000))))
    });

    // Lookups per second, from scopes of growing size: should stay flat.
    let lookup = scenario::compile(&engine, &scenario::lookup_loop());
    group.throughput(Throughput::Elements(1_000));
    for count in [4, 64, 1024] {
        let variables = scenario::many_variables(count, 1_000);
        group.bench_with_input(
            BenchmarkId::new("lookup", count),
            &variables,
            |b, variables| {
                b.iter_batched(
                    || scenario::options(variables.clone()),
                    |options| lookup.run_blocking(options),
                    criterion::BatchSize::SmallInput,
                )
            },
        );
    }
    group.finish();
}

/// JSON in and out, as a host storing rules as JSON would run them.
fn json(c: &mut Criterion) {
    let engine = scenario::engine();
    let rule_json = scenario::readme_rule_json();
    let row_json = r#"{"Name":"  Ada Lovelace  "}"#;
    let mut group = c.benchmark_group("json");
    group.throughput(Throughput::Bytes(rule_json.len() as u64));
    group.bench_function("parse", |b| {
        b.iter(|| codyscript_json::from_str::<codyscript_core::Value>(black_box(&rule_json)))
    });
    group.throughput(Throughput::Elements(1));
    group.bench_function("end_to_end", |b| {
        b.iter(|| scenario::json_end_to_end(&engine, black_box(&rule_json), row_json))
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = compile, run, json
}
criterion_main!(benches);
