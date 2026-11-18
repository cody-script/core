# CodyScript for Rust

CodyScript is a small, sandboxed interpreter for programs written as data.
The **host** decides what a program can do by handing it **executors**,
which provide named external functions; the **program** — stored config,
written by a person or an editor — supplies the logic: variables,
conditions, loops, functions of its own, and calls to those and to the
host's. Nothing else is reachable.

It is meant for rules that live in data rather than code: a tenant's CSV
mapping, a validation rule, a pipeline step. It began as a port of the
TypeScript package `@cody-script/core`, but has its own semantics, closer to
Rust's than JavaScript's: every call is a call of a
[function](crates/codyscript-core/README.md#functions), the host's or the
program's own. It has no `unsafe` code and works with any async runtime, or
none. MIT licensed.

## The crates

| Crate | What it is | Depends on |
|---|---|---|
| [`codyscript-core`](crates/codyscript-core) | The interpreter: data model (`Value`), validation, expressions, engine, debugger, errors. Knows no data format. | `serde` |
| [`codyscript-json`](crates/codyscript-json) | JSON: parse and print programs and values, the language's JSON Schema, `serde_json` interop. | `codyscript-core`, `serde_json` |
| [`codyscript-wasm`](crates/codyscript-wasm) | WebAssembly bindings: run JSON programs from JavaScript, with JavaScript executors (sync or async). Not published. | `codyscript-json`, `wasm-bindgen` |
| [`codyscript-playground`](crates/codyscript-playground) | A page for smoke-testing the WebAssembly build in a browser, served by `axum`. Not published. | `axum`, `wasm-bindgen-cli-support` |
| [`codyscript-bench`](crates/codyscript-bench) | Benchmarks: speed and memory, natively (with and without Tokio) and in WebAssembly. Not published. | `criterion`, `tokio` (dev) |

Dependencies point one way, inward:

```text
codyscript-json ──▶ codyscript-core ──▶ serde
      │                                   ▲
      └──────────▶ serde_json ────────────┘
```

The core owns its data model and implements serde's `Serialize` and
`Deserialize` for it, and nothing else. That is the whole contract a data
format needs: any serde format crate already reads and writes CodyScript
values. YAML, TOML, CBOR or MessagePack support is a matter of calling that
format's `from_str`/`to_vec` — or, for format-specific conveniences like
`codyscript-json` has (a schema, error mapping), a new adapter crate beside
it. The core never changes for a new format.

```toml
[dependencies]
codyscript-core = "0.9"
codyscript-json = "0.9"   # if your programs are stored as JSON
```

The crates need Rust 1.99 or newer (edition 2024).

## Quick start

A rule that cleans a CSV row's `Name` column and refuses a blank one:

```json
{
  "ext": { "text": ["trim", "empty"] },
  "do": [
    { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
    {
      "if": { "call": { "text.empty": { "value": { "ref": "name" } } } },
      "do": [{ "ret": { "val": { "error": "Name is required" } } }],
      "tag": "name-required"
    },
    { "ret": { "ref": "name" } }
  ]
}
```

`ext` declares the host functions the rule uses — here `text.trim` and
`text.empty`, which the host below provides — and `call` calls them. A
rule can also define functions of its own, in CodyScript.

```rust
use codyscript_core::{Engine, ExecutorCall, ExecutorResult, RunOptions, Value, Variables};
use codyscript_json::EngineJsonExt;

fn text(call: ExecutorCall) -> ExecutorResult {
    let value = call.input.get("value").and_then(Value::as_str).unwrap_or("");
    match call.op.as_str() {
        "trim" => Ok(value.trim().into()),
        "empty" => Ok(value.is_empty().into()),
        op => Err(format!("text has no \"{op}\"").into()),
    }
}

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let rule_json = r#"{ "ext": { "text": ["trim", "empty"] }, "do": [
#   { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
#   { "if": { "call": { "text.empty": { "value": { "ref": "name" } } } },
#     "do": [{ "ret": { "val": { "error": "Name is required" } } }], "tag": "name-required" },
#   { "ret": { "ref": "name" } } ] }"#;
# let rows_json = [r#"{ "Name": "  Ada " }"#];
let engine = Engine::builder().executor("text", ["trim", "empty"], text).build();

let rule = engine.compile_json(rule_json)?;                   // validate once…
for row in rows_json {
    let row: Variables = codyscript_json::from_str(row)?;
    let result = rule.run_blocking(RunOptions::from(row))?;   // …run per row
    println!("{}", codyscript_json::to_string(&result.value)?);
    # assert_eq!(result.value, "Ada");
}
# Ok(())
# }
```

`compile` checks the program's shape, depth, expression syntax, every call,
and that the host provides every function it declares, up front — so a
broken rule fails when it's saved, not halfway through a file. In async code, `rule.run(options).await` instead.

A host can also **debug** a rule — run it with a debugger,
`rule.run(options.with_debugger(breakpoints, debugger))` — which pauses it at breakpoints — any step can have one, named by its
tag or path — to show its variables and call stack, then steps over, into,
on or stops as the debugger says. A rule's variables and breakpoints can be
kept beside it, apart from the rule, as a *run configuration*: a JSON
document with [its own schema](crates/codyscript-json/run-config.schema.json). See the core README's
[Debugging](crates/codyscript-core/README.md#debugging), and try it in the
playground, below.

## WebAssembly

The core and JSON crates compile unchanged to `wasm32-unknown-unknown`.
`codyscript-wasm` exposes them to JavaScript, and is tested two ways: its
`wasm-bindgen-test` suite (in Node, or a browser), and the playground — a
page that runs a checklist in your browser:

```bash
rustup target add wasm32-unknown-unknown          # once
cargo run -p codyscript-playground                # then open http://127.0.0.1:8080
```

See [`codyscript-playground`](crates/codyscript-playground/README.md) for
what to check, and [`codyscript-wasm`](crates/codyscript-wasm/README.md) for
the JavaScript API.

## Performance

[`codyscript-bench`](crates/codyscript-bench/README.md) measures speed and
memory across compiling, running, JSON, executors and concurrency — natively,
under Tokio, and in single-threaded WebAssembly — with the results and what
they show.

## Documentation

- [`codyscript-core`](crates/codyscript-core/README.md): the language
  reference, semantics, host API, safety model and upgrade notes.
- [`codyscript-json`](crates/codyscript-json/README.md): reading and writing
  JSON, and the JSON Schema for editors.

## Development

```bash
cargo test --workspace            # unit, integration and doc tests (READMEs included)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo tree -p codyscript-core -e normal   # must show serde only

# WebAssembly (see above for the one-time setup)
cargo test -p codyscript-wasm --target wasm32-unknown-unknown   # in Node
cargo clippy -p codyscript-wasm --target wasm32-unknown-unknown --all-targets -- -D warnings
```
