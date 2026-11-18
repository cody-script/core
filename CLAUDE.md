# CodyScript for Rust

The **CodyScript interpreter**, as a Cargo workspace: programs are data, the
host supplies executors (which provide named external functions), and
nothing else is reachable. A port of the TypeScript package
`@cody-script/core` 0.5 (in `../corezip`) — but 0.6 changes the program
format ahead of it: every call is a `call` of a function, declared in the
root's `ext` (the host's) or defined in its `fn` (CodyScript's); `op` is
gone; codes `unknown_function`, `function_failed`, `depth_limit`. Listed in
the core README's differences and upgrade notes.
README.md is the overview; each crate's README is its rustdoc front page
(the core's is the language reference). This file is the working guide.

## Quick Reference

```bash
cargo test --workspace                                  # all tests, READMEs included
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo tree -p codyscript-core -e normal                 # must show serde only
cargo +1.99 test --locked --workspace                   # on the MSRV, as CI does

# WebAssembly: once, `rustup target add wasm32-unknown-unknown` and
# `cargo install wasm-bindgen-cli --version 0.2.130` (= the wasm-bindgen crate)
cargo test -p codyscript-wasm --target wasm32-unknown-unknown   # in Node
cargo run -p codyscript-playground                              # http://127.0.0.1:8080, "Run all"

# Benchmarks (see crates/codyscript-bench/README.md)
cargo bench -p codyscript-bench --bench speed    # also: --bench tokio, --bench memory
cargo bench -p codyscript-bench --target wasm32-unknown-unknown --bench wasm
```

## Architecture

```
crates/
  codyscript-core/   # The interpreter. Depends on serde only.
    src/
      lib.rs         # Public surface (keep it small); docs = README.md
      value/         # The data model: Value, Number, Map, value! macro,
                     #   and its serde impls (serde_impls.rs) — the only
                     #   place the core meets data formats
      engine.rs      # Engine, EngineBuilder, CompiledProgram (the public API)
      run.rs         # The interpreter: one loop over explicit stacks;
                     #   a function call is a frame on them, not recursion.
                     #   The loop is built twice (drive::<DEBUGGED>): only
                     #   a debugged run's may pause before a step
      config.rs      # RunConfig: variables and breakpoints, beside a program
      debug.rs       # Debugging (pub mod debug): Debugger, Action, Pause
                     #   (what a paused run shows), Frame, Breakpoint (by
                     #   tag or path) and checking them, the step list
                     #   (CompiledProgram::steps); Session, which decides
                     #   before each step whether a run pauses, pauses it,
                     #   and says whether it goes on
      validate.rs    # One walk: shape, finite numbers, depth, reserved keys,
                     #   root-only `fn`/`ext`, what each call names, the
                     #   host's functions, expression syntax; builds the tree
      program.rs     # The validated program tree (crate-private): a
                     #   Program is its body and its `fn` functions; a call
                     #   is resolved to an External or a function index
      expression.rs  # The `x` language: tokenizer, parser, evaluator
      executor.rs    # Executor trait, ExecutorCall, Signal, closure adapters
                     #   (engine.rs registers each with its function names)
      error.rs       # Error, ErrorCode, Issue, Details
    tests/
      common/mod.rs  # Builders, the `math` executor, valid/invalid fixtures
                     #   (shared with codyscript-json's tests via #[path])
      engine.rs, validate.rs, expression.rs  # ports of the TypeScript tests
      debug.rs       # pausing, stepping, breakpoints, the step list
      data.rs        # format independence: serde tokens, CBOR end to end
  codyscript-json/   # The JSON adapter. Depends on core + serde_json.
    src/lib.rs       # from_str/to_string…, EngineJsonExt::compile_json,
                     #   serde_json interop, SCHEMA, RUN_CONFIG_SCHEMA; runs
                     #   README.md's doc tests
    schema.json      # JSON Schema for hand-writing programs
    run-config.schema.json  # JSON Schema for run configurations (IDEs)
    tests/           # json.rs (conventions), schema.rs, readme.rs
  codyscript-wasm/   # WebAssembly bindings (wasm-bindgen); not published.
    src/lib.rs       # compile(program, executors, limits) → Program, and
                     #   run(program, variables, executors, limits), for
                     #   JavaScript; executors are objects of JavaScript
                     #   functions ({ text: { trim } }), bridged via send_wrapper;
                     #   debug(program, config, …, onPause), steps(…),
                     #   check(…) and runConfigSchema() for debugging
    tests/web.rs     # wasm-bindgen-test: Node by default, browsers too
    npm/             # the npm package @cody-script/core-wasm: build.sh,
                     #   package.json template, README, smoke.mjs (Node)
  codyscript-playground/  # A browser smoke test for codyscript-wasm; not published.
    src/main.rs      # what it's for, arguments, startup
    src/wasm.rs      # builds codyscript-wasm (cargo) + its glue (in memory)
    src/server.rs    # serves page, checklist and module (axum)
    src/cases.rs     # the checklist, in Rust: add cases here (a case may
                     #   script a debugger — actions, and the pauses it
                     #   must make — for its run configuration's breakpoints;
                     #   `expect` is only the outcome)
    src/index.html   # the page: demo executors, run, compare, show;
                     #   the debugger (steps, breakpoints, stack, variables)
  codyscript-bench/  # Benchmarks; not published.
    src/lib.rs       # the scenarios every benchmark shares
    src/alloc.rs     # a counting global allocator, for memory
    benches/         # speed.rs, tokio.rs, memory.rs (native, Criterion);
                     #   wasm.rs (wasm-bindgen-test's Criterion, in Node)
.cargo/config.toml   # wasm-bindgen-test-runner as the wasm32 test runner
.github/workflows/   # ci.yml (PRs); release.yml (main: CI, then crates.io,
                     #   GitHub release, npm package on GitHub Packages)
```

**The dependency rule: dependencies point inward.** The core owns the data
model and knows no format; it may depend on `serde` and nothing else at
runtime (dev-dependencies are fine). Format crates depend on the core, never
the reverse. A new format is a new adapter crate beside `codyscript-json` —
or nothing at all, since any serde format already reads and writes `Value`.
If a change to the core needs a format, the design is wrong: move that part
to an adapter. JSON-specific things (the schema, `serde_json` errors and
interop, recursion limits) live in `codyscript-json`.

Other constraints: no async runtime dependency (`run` is a plain future;
`run_blocking` uses a std-only `block_on`); `#![forbid(unsafe_code)]`.

**Toolchain: one Rust for everything.** Edition 2024, and `rust-version` in
`Cargo.toml` (1.99) is the one MSRV for every crate, its tests, benchmarks
and tools — CI's `msrv` job reads it from there and runs the whole suite on
it with the committed lockfile. Use current Rust freely (let chains,
`pop_if`, lint `reason`s, …); when you need something newer, raise
`rust-version` (and note it in the core README's upgrade notes) rather than
working around it. Resolver 3 keeps `cargo update` to dependency versions
that support it. Not yet possible on stable, so don't try: executors as
`AsyncFn` closures that capture state (`async move |call| …`) — a `Send`
bound on their future needs unstable features; non-capturing `async |call|`
closures already work with `async_executor`.

## Rules

- **Programs are untrusted.** Every new node type needs: its fields in
  `INPUTS`/`INSTRUCTIONS` and a case in `validate.rs`, a variant in
  `program.rs`, a case in the engine with a `step()` per evaluation, a
  definition in `codyscript-json/schema.json`, and cases in the core's
  `tests/common/mod.rs` (run against both the validator and the schema).
  For the debugger, it also needs a `NodeKind` (and `Node::kind`/`name` in
  `debug.rs`) and its parts in `push_parts`, in the order they run; a node
  is a step a debugger can pause at only if a `Task::Resolve`/`Execute`
  runs it (`Task::step`). A plain run executes a list's first instruction
  and resolves an instruction's input at once; a debugged run pushes them
  as those tasks instead (`list`/`execute::<DEBUGGED>`) — keep any new
  shortcut like that behind `DEBUGGED`.
- **No stack overflows from programs.** Running never recurses: `run.rs` is
  one loop over explicit stacks (tasks, values, block results, scopes), and
  expressions are a flat arena evaluated the same way. Only compiling
  (validation, the tree, cloning and dropping values) recurses, at most
  `max_depth` deep — every container is depth-checked before it's
  descended. Don't add recursion over anything a program controls.
  `tests/engine.rs` runs the deepest default-limit programs on a 1 MiB
  stack (WebAssembly's), and `codyscript-wasm` checks it under wasm32.
  Listing a program's steps (`debug::steps`) walks an explicit stack too.
- **Debugging costs plain runs nothing.** `drive` is generic over
  `DEBUGGED`, so the loop of a run that isn't debugged is compiled without
  the pause; keep anything a pause holds across an `.await` inside
  `debug_task`, not `drive`, or every run pays to keep it. Pausing —
  whether, the `Pause`, the debugger, what its action means — is
  `Session::before`'s business (in `debug.rs`), not the loop's; the loop
  only asks it, before each step, whether to go on. Anything else the
  debugger needs is state a run keeps anyway (the `invocations` call
  stack). Debugged runs may be slower — a `Pause` is built fresh, as plain
  data, and the session indexes the program's steps when it starts — but
  plain runs may not. A run pauses only at breakpoints and where its debugger's actions
  take it — never of its own accord. A debugged run must behave
  exactly as a plain one between pauses — same result, steps, errors — and
  a `Pause` is read-only. Timings vary too much on shared machines to show
  a few percent: compare instruction counts (Valgrind's cachegrind) before
  and after touching the loop.
- **Everything called is a function.** The root node — any input — may
  carry `fn` (CodyScript functions, each a program itself) and `ext` (the
  host's functions it uses, by executor: `{ "text": ["trim"] }`); both
  only there. `call` calls either; a name is in one or the other, never
  both; only external calls take `arg`. Hosts register each executor with
  the functions it provides, so `compile` checks `ext` exactly
  (`unknown_function`) — keep that: a program's `ext` is its whole reach
  outside the sandbox. A CodyScript call runs in a fresh scope holding only
  its arguments (`Machine::base` stops reads falling through to the
  caller's); nesting is bounded by `max_depth` at run time (`depth_limit`),
  which also bounds the memory recursion can take.
- **Browser WebAssembly is single-threaded and JavaScript values aren't
  `Send`.** Bridge them in `codyscript-wasm` (`send_wrapper`), never by
  loosening the core's `Send` bounds.
- **Tools live in their own crates.** The playground (axum, tokio,
  wasm-bindgen-cli-support) and the benchmarks (criterion, tokio, a counting
  allocator) are separate, unpublished crates, so none of their dependencies
  reach the library crates. The playground doesn't depend on
  `codyscript-wasm`; it builds it. `wasm-bindgen` and
  `wasm-bindgen-cli-support` are pinned together in the workspace.
- **The playground stays small and mostly Rust.** Checks go in `cases.rs`
  (Rust data); the page's JavaScript only defines the demo executors, runs,
  compares and renders — and drives the debugger, whose steps come from the
  engine (`steps`), not from walking the program in JavaScript.
- **Benchmarks share their scenarios** (`codyscript-bench/src/lib.rs`) and
  WebAssembly benchmark names match native ones, so results compare line by
  line. Memory benchmarks borrow the scenario (`measure(&scenario)`):
  dropping it mustn't count as the work. Update the results in the bench
  README when numbers move materially, with the machine they came from.
- **The program format is a contract** shared with the TypeScript package
  and stored rules. Messages, paths and issue order match it exactly (a
  differential fuzz against the TypeScript engine found no differences);
  change them only together, with a version bump and an upgrade note.
- **Run configurations are a format too**, for IDEs: a program's
  `variables` and `breakpoints`, kept apart from it. `RunConfig`'s serde
  impls and `run-config.schema.json` accept and refuse the same documents
  (`codyscript-json/tests/schema.rs`; hence `RunConfig`'s hand-written
  `Deserialize`, which takes objects only). It holds no limits — those are
  the host's. A breakpoint names steps by `tag` (preferred: edits don't
  move it) or `path`, and one that names no step is an `invalid_config`
  error, whenever breakpoints are set — never silently ignored.
- **Values are format-neutral.** `Number` holds any `f64` (formats like CBOR
  carry `NaN`); `compile` refuses non-finite numbers in programs. `Map`
  keeps insertion order — it decides argument order — and compares
  order-insensitively.
- **Expressions stay small**: arithmetic, comparison and logic; no calls,
  assignment or loops. Strict types (`1 + 'a'` is a `type_error`). Numbers
  are f64 as in JavaScript; whole results become integers.
- **No global state.** Executors belong to an engine.
- **Errors are `codyscript_core::Error`** with a code, a JSON Pointer path
  and the node's tag. Adding an `ErrorCode` is a public API change —
  document it in the core README.
- The top-level README's first `json` block is executed by
  `codyscript-json/tests/readme.rs`; every README's `rust` blocks are doc
  tests. Keep them runnable.
- Known differences from the TypeScript engine are listed in the core
  README ("Differences from the TypeScript package"); keep that list
  accurate.

Keep this file accurate. An out-of-date CLAUDE.md is worse than none.
