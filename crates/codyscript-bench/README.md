# codyscript-bench

Benchmarks for CodyScript: how fast it compiles and runs, and what it
allocates — natively, under Tokio, and in single-threaded WebAssembly. Every
benchmark measures the same scenarios (`src/lib.rs`), so the numbers compare
like for like. Timings use [Criterion](https://docs.rs/criterion) (natively)
and wasm-bindgen-test's port of it (in WebAssembly); memory is counted by a
global allocator (`src/alloc.rs`).

It's a separate crate so that Criterion, Tokio and the counting allocator
never become dependencies of the library crates.

## Running

```bash
# native (each is `harness = false`, so Criterion's flags work after `--`)
cargo bench -p codyscript-bench --bench speed     # compile, run, JSON
cargo bench -p codyscript-bench --bench tokio     # async executors and concurrency
cargo bench -p codyscript-bench --bench memory    # allocations: a Markdown table

# WebAssembly, in Node (one-time setup: `rustup target add
# wasm32-unknown-unknown`, `cargo install wasm-bindgen-cli --version 0.2.130`)
cargo bench -p codyscript-bench --target wasm32-unknown-unknown --bench wasm
cargo bench -p codyscript-bench --target wasm32-unknown-unknown --bench wasm -- memory   # just the table
```

Use `--bench` for WebAssembly: the native benchmarks don't build for wasm32.
Filter with a name after `--` (`cargo bench -p codyscript-bench --bench speed
-- run/sum_loop`). To check a change for regressions, save a baseline first
and compare against it:

```bash
cargo bench -p codyscript-bench --bench speed -- --save-baseline before
# …change something…
cargo bench -p codyscript-bench --bench speed -- --baseline before
```

## What's measured

| Benchmark | Scenario |
|---|---|
| `compile/readme`, `compile/fee` | Compiling the README's rule (two external calls) and an expressions-only rule. |
| `compile/deep` | The deepest program the default `max_depth` allows (42 nested blocks). |
| `compile/wide/{10,100,1000}` | Programs of that many instructions, each with its own expression: compile cost per instruction. |
| `run/readme`, `run/fee`, `run/deep` | Running those, once compiled. |
| `run/sum_loop/{10…10000}` | A loop of expressions: throughput in steps per second, as programs grow. |
| `run/call_loop/1000` | 1,000 calls to a synchronous executor: the cost of a call. |
| `run/function_loop/1000` | 1,000 calls to a CodyScript function (`fn`): the cost of a call with a fresh scope. |
| `run/lookup/{4,64,1024}` | 1,000 variable reads from a scope of that many variables. |
| `json/parse`, `json/end_to_end` | Parsing the README rule; parse, compile, run and print it, as a JSON host does per row. |
| `tokio/executor/…` | One run making 100 executor calls: `blocking` (`run_blocking`), on Tokio's `current_thread` or `multi_thread` runtime, or on the JavaScript `event_loop` (WebAssembly) — with `sync` executors, `async_ready` ones (async, but never waiting) and `yielding` ones (suspending once per call, as I/O would; the same code on both runtimes). |
| `tokio/concurrency/…` | 64 such runs of one compiled program: `sequential`, or spawned on `current_thread` / `multi_thread`, or interleaved on the `event_loop` (WebAssembly). |
| `wasm/js_executor/{sync,promise}` | WebAssembly only: 100 calls to a JavaScript executor — a plain function, or one returning a promise — through `codyscript-wasm`'s bindings, JSON in and out. |
| memory | Allocations, bytes, peak and retained memory for compiling, running and JSON scenarios, plus (natively) 64 Tokio runs one by one and concurrently. |

WebAssembly names match the native ones, so the two reports line up.

## Results

From the machine this was written on: 4 vCPUs (Intel Xeon, 2.6 GHz), Rust
1.99, Node 22. Two things limit what these numbers can say:

- **Timings vary ±20–50% between runs here.** It's a shared virtual machine;
  compare orders of magnitude and ratios, not single digits, and benchmark
  on the hardware you deploy to.
- **It has no real parallelism.** Two threads doing twice the work take 3.7×
  as long as one thread doing half (see `multi_thread` below), so the
  multi-threaded rows measure contention, not what four cores would give.

### Speed

| Benchmark | Native | WebAssembly (Node) | Ratio |
|---|--:|--:|--:|
| `compile/readme` | 12 µs | 22 µs | 1.9× |
| `compile/fee` | 9.0 µs | 19 µs | 2.1× |
| `compile/deep` | 109 µs | 136 µs | 1.2× |
| `compile/wide/100` | 379 µs (264 K instructions/s) | 572 µs | 1.5× |
| `run/readme` | 2.1 µs | 3.8 µs | 1.8× |
| `run/fee` | 1.4 µs | 2.4 µs | 1.8× |
| `run/sum_loop/1000` | 1.1 ms (12 M steps/s) | 1.6 ms | 1.4× |
| `run/call_loop/1000` | 1.0 ms (1 M calls/s) | 1.7 ms | 1.6× |
| `run/function_loop/1000` | 0.61 ms (1.6 M calls/s) | 0.84 ms | 1.4× |
| `json/parse` | 5.2 µs (41 MiB/s) | 5.2 µs | 1.0× |
| `json/end_to_end` | 17 µs | 24 µs | 1.5× |

`run/function_loop` was measured in a later session, in which
`run/call_loop` took 0.54 ms natively and 0.91 ms in WebAssembly: a call of a
CodyScript function, fresh scope and all, costs about what a call of a
synchronous external one does.

Natively, `run/sum_loop` holds 11–12 M steps/s from 10 to 10,000 passes, and
`run/lookup` 1.0–1.2 M reads/s whether the scope has 4 variables or 1,024.
`compile/wide` slows from 290 K to 200 K instructions/s between 10 and
1,000 instructions.

### Executors and concurrency

One run, 100 executor calls:

| | Native | | WebAssembly | |
|---|--:|---|--:|---|
| `sync` | 92 µs | blocking, Tokio current-thread and multi-thread alike | 165–205 µs | blocking or on the event loop |
| `async_ready` | 92–119 µs | | 175–185 µs | |
| `yielding` | 125–130 µs | | 334 µs | |
| JavaScript executor | | | 641 µs (sync), 1.9 ms (promise) | through the bindings |

64 runs of 100 calls each:

| | Native, sequential | Native, spawned on `current_thread` | Native, `multi_thread` | WebAssembly, sequential | WebAssembly, interleaved |
|---|--:|--:|--:|--:|--:|
| `sync` | 9.0 ms | 7.2 ms | 20.6 ms (see above) | 10.3 ms | 10.6 ms |
| `yielding` | 8.5 ms | 9.2 ms | 15.4 ms (see above) | 20.4 ms | 12.4 ms |

### Memory

Native (64-bit); WebAssembly's counts are identical and its bytes about a
third smaller (32-bit pointers):

| Scenario | Allocations | Allocated | Peak | Retained |
|---|--:|--:|--:|--:|
| parse the README rule from JSON | 44 | 7.1 KiB | 7.1 KiB | 7.1 KiB |
| compile the README rule | 226 | 13.2 KiB | 11.0 KiB | 4.3 KiB |
| compile a wide program (100 instructions) | 4709 | 404.9 KiB | 254.9 KiB | 132.3 KiB |
| compile the deepest default program | 1259 | 151.0 KiB | 93.0 KiB | 38.8 KiB |
| run the README rule | 25 | 2.8 KiB | 1.8 KiB | 584 B |
| run the sum loop, n = 1000 | 11014 | 521.4 KiB | 1.5 KiB | 541 B |
| run the call loop, n = 1000, sync executor | 10013 | 765.5 KiB | 1.9 KiB | 538 B |
| run the call loop, n = 1000, async executor | 10013 | 874.9 KiB | 1.9 KiB | 538 B |
| JSON in, run, JSON out (README rule) | 296 | 23.1 KiB | 13.1 KiB | 152 B |
| Tokio: 64 runs of the call loop (n = 100), one by one | 64704 | 4.9 MiB | 1.8 KiB | 0 B |
| Tokio: 64 runs of the call loop (n = 100), concurrently | 64769 | 4.9 MiB | 58.7 KiB | 0 B |

"Retained" is the size of what the scenario returns: a compiled program's
footprint, or a run's result. The WebAssembly table also reports the
module's linear memory, which includes the benchmark harness's own buffers.

### What they show

- **Running is linear and constant-memory.** Steps per second don't fall as
  loops grow, and a run's peak memory stays under 2 KiB however many steps
  it takes. Variable lookups don't slow down with the size of the scope.
- **Runs allocate a lot of short-lived memory**: about 11 allocations per
  loop pass (0.8 per step), for values, variable names and executor inputs.
  It's the clearest target for optimization.
- **A compiled program costs about 1.3 KiB per instruction** (132 KiB for
  100 instructions with an expression each). About 28% of that is the copy
  of the source value kept for `CompiledProgram::program()`; the rest is the
  program tree and each expression's parsed form. (The JSON Pointer paths
  kept for error messages are a small part, about 3.5 KiB.)
- **WebAssembly in Node runs at 1.2–2× native time**, close to native for
  large compiles and JSON parsing.
- **Async costs little natively.** A yielding executor adds about 0.4 µs per
  suspension on Tokio. On the JavaScript event loop a suspension costs
  about 1.3 µs, and interleaving runs pays: 64 yielding runs finish in 12 ms
  interleaved against 20 ms one by one.
- **JavaScript executors are dominated by the boundary**: a call costs
  about 6 µs in all, crossing as JSON each way, against under 2 µs for a
  Rust executor in the same module; a promise doubles it. Keep hot logic in
  Rust or in expressions.
- **Concurrent runs cost memory in proportion**: 64 in flight peak at about
  59 KiB, under 1 KiB each.
