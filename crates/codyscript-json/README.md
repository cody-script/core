# codyscript-json

JSON for [CodyScript](../../README.md): read and write programs, variables
and results as JSON, compile JSON text directly, and check hand-written
programs against the language's JSON Schema.

`codyscript-core` is the interpreter and knows no data format; this crate is
its JSON adapter, built on `serde_json`. Everything here could be done with
`serde_json` and the core's serde impls alone — this crate names the JSON
conventions once (document-order keys, `NaN` written as `null`, syntax errors
as `invalid_program`) so every host gets them the same way.

```rust
use codyscript_core::{Engine, RunOptions, Variables};
use codyscript_json::EngineJsonExt;

let engine = Engine::new();
let fee = engine.compile_json(r#"{ "do": [
    { "set": { "fee": { "x": "amount * 0.03" } } },
    { "if": { "x": "fee > 300" }, "do": [{ "set": { "fee": 300 } }] },
    { "ret": { "ref": "fee" } }
] }"#).unwrap();

let row: Variables = codyscript_json::from_str(r#"{ "amount": 20000 }"#).unwrap();
let result = fee.run_blocking(RunOptions::from(row)).unwrap();
assert_eq!(codyscript_json::to_string(&result).unwrap(),
           r#"{"value":300,"variables":{"amount":20000,"fee":300},"steps":14}"#);
```

## Reading and writing

- `from_str`, `from_slice`, `from_reader` parse JSON into a `Value` (a
  program, any value), `Variables` (must be an object) or any
  `Deserialize` type. Objects keep their keys in document order; a
  repeated key keeps its first place and its last value, as `JSON.parse`
  does. Nesting is limited to 128 levels, the default `max_depth`.
- `to_string`, `to_string_pretty`, `to_vec`, `to_writer` write a `Value`, a
  `RunResult` (`{ value, variables, steps }`), an `Error`
  (`{ code, message, path, tag?, issues? }`) or anything serializable.
  Numbers JSON can't hold (`NaN`, `Infinity`) are written as `null`, as
  `JSON.stringify` does.
- `engine.compile_json(text)` (from `EngineJsonExt`) parses and compiles in
  one go. Text that isn't JSON fails like any malformed program:
  `invalid_program`, with the parser's message (saying where) and the
  parser's error as its `cause()`.
- `from_serde_json` and `to_serde_json` convert to and from
  `serde_json::Value` for code that already has one.

## Writing programs by hand

[`schema.json`](schema.json) (also `codyscript_json::SCHEMA`) is a JSON
Schema for the language. Point your editor at it for completion and inline
errors — in VS Code, `.vscode/settings.json`:

```jsonc
{ "json.schemas": [{ "fileMatch": ["*.cody.json"], "url": "./schema.json" }] }
```

It checks shape only; `compile` also checks executor ids, nesting depth,
expression syntax and that numbers are finite. A test keeps the two in
agreement.

## Storing programs

Store programs minified, and compress them if size matters: gzip shrinks a
typical rule by around 40% (the fee example in `codyscript-core`'s README
goes from 272 bytes to 163).
