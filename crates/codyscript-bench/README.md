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
| `debug/sum_loop/1000` | `run/sum_loop/1000` under a debugger, with a breakpoint to check at every step that pauses it only at the very end: what debugging costs a run between pauses. |
| `json/parse`, `json/end_to_end`, `json/compiled_once` | Parsing the README rule; parsing, compiling (`compile_json`), running and printing it for a row; and a row's cost when the rule is compiled once: parse the row, run, print. |
| `tokio/executor/…` | One run making 100 executor calls: `blocking` (`run_blocking`), on Tokio's `current_thread` or `multi_thread` runtime, or on the JavaScript `event_loop` (WebAssembly) — with `sync` executors, `async_ready` ones (async, but never waiting) and `yielding` ones (suspending once per call, as I/O would; the same code on both runtimes). |
| `tokio/concurrency/…` | 64 such runs of one compiled program: `sequential`, or spawned on `current_thread` / `multi_thread`, or interleaved on the `event_loop` (WebAssembly). |
| `wasm/js_executor/{sync,promise}` | WebAssembly only: 100 calls to a JavaScript executor — a plain function, or one returning a promise — through `codyscript-wasm`'s bindings, JSON in and out. |
| `wasm/readme/{run,compiled_once}` | WebAssembly only: the README rule through the bindings, with its executor in JavaScript — `run` (compiling for every row), or `compile` once, then `program.run` per row. |
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
| `compile/readme` | 9.1 µs | 17 µs | 1.8× |
| `compile/fee` | 7.5 µs | 13 µs | 1.8× |
| `compile/deep` | 92 µs | 104 µs | 1.1× |
| `compile/wide/100` | 359 µs (279 K instructions/s) | 414 µs | 1.2× |
| `run/readme` | 2.0 µs | 2.9 µs | 1.4× |
| `run/fee` | 1.4 µs | 1.9 µs | 1.4× |
| `run/sum_loop/1000` | 0.80 ms (17 M steps/s) | 1.2 ms | 1.5× |
| `run/call_loop/1000` | 0.80 ms (1.25 M calls/s) | 1.4 ms | 1.8× |
| `run/function_loop/1000` | 0.61 ms (1.6 M calls/s) | 0.84 ms | 1.4× |
| `debug/sum_loop/1000` | 1.1–1.6 ms | 2.0 ms | 1.3–1.8× |
| `json/parse` | 5.0 µs (43 MiB/s) | 7.2 µs | 1.5× |
| `json/end_to_end` | 15 µs | 26 µs | 1.7× |
| `json/compiled_once` | 2.6 µs | 4.0 µs | 1.6× |

`debug/sum_loop` was measured in an earlier session too, before the run
loop's allocations were removed, beside `run/sum_loop/1000`
at 0.8–1.3 ms natively and 0.95 ms in WebAssembly: between pauses, a
debugged run is 1.3–1.5× slower natively (24% more instructions, counted
with Valgrind, whose counts don't vary as timings do here) and 2× in
WebAssembly. The debugger's code favors clarity over speed — the run asks
its debugging session before every step — since a debugged run spends its
time paused anyway. Runs that aren't debugged don't pay for it: their
instruction counts didn't change when it was added.

`run/function_loop` was measured in an earlier session, before the run
loop's allocations were removed, in which `run/call_loop` took 0.54 ms
natively and 0.91 ms in WebAssembly: a call of a CodyScript function, fresh
scope and all, costs about what a call of a synchronous external one does.

Natively, `run/sum_loop` holds 15–17 M steps/s from 10 to 10,000 passes, and
`run/lookup` 1.5–1.7 M reads/s whether the scope has 4 variables or 1,024.
`compile/wide` holds 220–280 K instructions/s from 10 to 1,000
instructions.

### Executors and concurrency

One run, 100 executor calls:

| | Native | | WebAssembly | |
|---|--:|---|--:|---|
| `sync` | 87–101 µs | blocking, Tokio current-thread and multi-thread alike | 130–157 µs | blocking or on the event loop |
| `async_ready` | 81–90 µs | | 132–138 µs | |
| `yielding` | 91–108 µs | | 274 µs | |
| JavaScript executor | | | 548 µs (sync), 1.2 ms (promise) | through the bindings |

The README rule through the bindings, its executor in JavaScript: 36 µs a
row with `run`, 25 µs with `compile` once and `program.run` per row (the
rest is its two calls crossing the boundary).

64 runs of 100 calls each:

| | Native, sequential | Native, spawned on `current_thread` | Native, `multi_thread` | WebAssembly, sequential | WebAssembly, interleaved |
|---|--:|--:|--:|--:|--:|
| `sync` | 5.5 ms | 5.6 ms | 7.0 ms (see above) | 8.1 ms | 9.7 ms |
| `yielding` | 7.3 ms | 8.8 ms | 12.4 ms (see above) | 16.5 ms | 9.3 ms |

### Memory

Native (64-bit); WebAssembly's counts are identical and its bytes about a
third smaller (32-bit pointers):

| Scenario | Allocations | Allocated | Peak | Retained |
|---|--:|--:|--:|--:|
| parse the README rule from JSON | 50 | 4.3 KiB | 4.3 KiB | 4.3 KiB |
| compile the README rule | 44 | 2.0 KiB | 1.7 KiB | 1.6 KiB |
| compile a wide program (100 instructions) | 1009 | 110.0 KiB | 50.9 KiB | 47.5 KiB |
| compile the deepest default program | 169 | 31.1 KiB | 30.9 KiB | 23.4 KiB |
| run the README rule | 15 | 1.3 KiB | 980 B | 328 B |
| run the sum loop, n = 1000 | 8 | 1.1 KiB | 888 B | 288 B |
| run the call loop, n = 1000, sync executor | 1008 | 94.9 KiB | 984 B | 288 B |
| run the call loop, n = 1000, async executor | 2008 | 165.2 KiB | 1.0 KiB | 288 B |
| JSON in, run, JSON out (README rule) | 108 | 7.5 KiB | 6.0 KiB | 152 B |
| JSON in, run, JSON out (README rule, compiled once) | 16 | 1.3 KiB | 953 B | 152 B |
| Tokio: 64 runs of the call loop (n = 100), one by one | 6848 | 667.5 KiB | 984 B | 0 B |
| Tokio: 64 runs of the call loop (n = 100), concurrently | 6915 | 724.1 KiB | 77.4 KiB | 112 B |

"Retained" is the size of what the scenario returns: a compiled program's
footprint, or a run's result. The WebAssembly table also reports the
module's linear memory, which includes the benchmark harness's own buffers.
The compile and JSON scenarios measure their own work alone: their
programs, and the rule's JSON text, are built before they start. (Until
0.10 they were built inside, which counted every node of the program as
compiling's work.)

### What they show

- **Running is linear and constant-memory.** Steps per second don't fall as
  loops grow, and a run's peak memory stays around 1 KiB however many steps
  it takes. Variable lookups don't slow down with the size of the scope.
- **Expressions and assignments don't allocate as they run**: the sum loop
  makes 9 allocations in all, however many passes it takes. A call of a
  synchronous executor makes 1: its input map's entries. (Until 0.10 it
  made 5: the function's name and each argument's name were copied for
  each call, and its result boxed.)
- **Compile once.** Parsing and compiling the README rule costs about five
  times what running it does: a JSON host pays 15 µs and 108 allocations a
  row if it compiles the rule each time, 2.6 µs and 16 if it compiles it
  once. Parsing itself is mostly building the value tree (serde_json reads
  the rule's text in well under 1 µs), so a faster parser or a binary
  format would save little.
- **A compiled program costs about 0.45 KiB per instruction** (45 KiB for
  100 instructions with an expression each): the program tree and each
  expression's parsed form. (The JSON Pointer paths kept for error messages
  are a small part, about 3.5 KiB.) Until 0.10 it also kept a copy of the
  source, about 30% more.
- **0.10 is faster than the speed table above shows.** Measured A/B
  against 0.9 on an Apple M4 Pro, alternating builds to cancel out a busy
  machine: compiling is 32–37% faster, with less than half the allocations
  (45 against 98 for the README rule, measuring compiling alone), since it
  no longer copies the program and builds a node's error paths only when
  there's an error to report. Runs that call executors are 20–35% faster
  (`run/call_loop` 35%, `run/readme` 21%, `json/compiled_once` 21%), and
  CodyScript function calls 12%: a call shares the function's name and its arguments' names (`Map`
  keys are `Arc<str>`) rather than copying them, a synchronous executor's
  result isn't boxed, and arguments aren't checked for duplicates they
  can't have. Scopes past 8 variables are about 5% slower to read
  (`run/lookup/64`), where a third of the time is hashing the name.
  Loops of expressions are 9–15% faster (`run/sum_loop`, `run/lookup/4`):
  short variable names are compared in place, and the evaluator's inline
  stacks are smaller.
- **WebAssembly in Node runs at 1.1–1.8× native time**, close to native for
  large compiles.
- **Async costs little natively.** A yielding executor adds about 0.2 µs per
  suspension on Tokio. On the JavaScript event loop a suspension costs
  about 1.4 µs, and interleaving runs pays: 64 yielding runs finish in 9.3 ms
  interleaved against 16.5 ms one by one.
- **JavaScript executors are dominated by the boundary**: a call costs
  about 5.5 µs in all, crossing as JSON each way, against under 2 µs for a
  Rust executor in the same module; a promise doubles it. Keep hot logic in
  Rust or in expressions.
- **Concurrent runs cost memory in proportion**: 64 in flight peak at
  45–65 KiB, about 1 KiB each.
