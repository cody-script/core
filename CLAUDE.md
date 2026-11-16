# CodyScript for Rust

The **CodyScript interpreter**, as a Cargo workspace: programs are data, the
host supplies executors, and nothing else is reachable. A port of the
TypeScript package `@cody-script/core` 0.5 (in `../corezip`): same program
format, semantics, error codes, paths and messages.
README.md is the overview; each crate's README is its rustdoc front page
(the core's is the language reference). This file is the working guide.

## Quick Reference

```bash
cargo test --workspace                                  # all tests, READMEs included
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo tree -p codyscript-core -e normal                 # must show serde only

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
      run.rs         # The interpreter: one loop over explicit stacks
      validate.rs    # One walk: shape, finite numbers, depth, reserved keys,
                     #   executor ids, expression syntax; builds the tree
      program.rs     # The validated program tree (crate-private)
      expression.rs  # The `x` language: tokenizer, parser, evaluator
      executor.rs    # Executor trait, ExecutorCall, Signal, closure adapters
      error.rs       # Error, ErrorCode, Issue, Details
    tests/
      common/mod.rs  # Builders, the `math` executor, valid/invalid fixtures
                     #   (shared with codyscript-json's tests via #[path])
      engine.rs, validate.rs, expression.rs  # ports of the TypeScript tests
      data.rs        # format independence: serde tokens, CBOR end to end
  codyscript-json/   # The JSON adapter. Depends on core + serde_json.
    src/lib.rs       # from_str/to_string…, EngineJsonExt::compile_json,
                     #   serde_json interop, SCHEMA; runs README.md's doc tests
    schema.json      # JSON Schema for hand-writing programs
    tests/           # json.rs (conventions), schema.rs, readme.rs
  codyscript-wasm/   # WebAssembly bindings (wasm-bindgen); not published.
    src/lib.rs       # run(program, variables, executors, limits) for JavaScript;
                     #   JavaScript executors via send_wrapper
    tests/web.rs     # wasm-bindgen-test: Node by default, browsers too
    npm/             # the npm package @cody-script/core-wasm: build.sh,
                     #   package.json template, README, smoke.mjs (Node)
  codyscript-playground/  # A browser smoke test for codyscript-wasm; not published.
    src/main.rs      # what it's for, arguments, startup
    src/wasm.rs      # builds codyscript-wasm (cargo) + its glue (in memory)
    src/server.rs    # serves page, checklist and module (axum)
    src/cases.rs     # the checklist, in Rust: add cases here
    src/index.html   # the page: demo executors, run, compare, show
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
`run_blocking` uses a std-only `block_on`); `#![forbid(unsafe_code)]`;
MSRV 1.77 (check with `cargo +1.77` on a lockfile resolved with
`CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback`).

## Rules

- **Programs are untrusted.** Every new node type needs: its fields in
  `INPUTS`/`INSTRUCTIONS` and a case in `validate.rs`, a variant in
  `program.rs`, a case in the engine with a `step()` per evaluation, a
  definition in `codyscript-json/schema.json`, and cases in the core's
  `tests/common/mod.rs` (run against both the validator and the schema).
- **No stack overflows from programs.** Running never recurses: `run.rs` is
  one loop over explicit stacks (tasks, values, block results, scopes), and
  expressions are a flat arena evaluated the same way. Only compiling
  (validation, the tree, cloning and dropping values) recurses, at most
  `max_depth` deep — every container is depth-checked before it's
  descended. Don't add recursion over anything a program controls.
  `tests/engine.rs` runs the deepest default-limit programs on a 1 MiB
  stack (WebAssembly's), and `codyscript-wasm` checks it under wasm32.
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
  compares and renders.
- **Benchmarks share their scenarios** (`codyscript-bench/src/lib.rs`) and
  WebAssembly benchmark names match native ones, so results compare line by
  line. Memory benchmarks borrow the scenario (`measure(&scenario)`):
  dropping it mustn't count as the work. Update the results in the bench
  README when numbers move materially, with the machine they came from.
- **The program format is a contract** shared with the TypeScript package
  and stored rules. Messages, paths and issue order match it exactly (a
  differential fuzz against the TypeScript engine found no differences);
  change them only together, with a version bump and an upgrade note.
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
