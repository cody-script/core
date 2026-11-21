# codyscript-wasm

CodyScript compiled to WebAssembly: run JSON programs from JavaScript, in a
browser or Node, with external functions written in JavaScript — plain or
`async` ones.

```js
import init, { compile, run } from "./pkg/codyscript_wasm.js";
await init();

const executors = {
  text: { trim: ({ input }) => String(input.value ?? "").trim() },
  api: { lookup: async ({ input }) => (await fetch(`/api/${input.id}`)).json() },
};
// A program declares what it uses: { "ext": { "text": ["trim"] }, … },
// and calls it: { "call": { "text.trim": { "value": { "ref": "Name" } } } }.
const program = compile(programJson, executors, '{"max_steps":10000}'); // once…
for (const row of rows) {
  const outcome = JSON.parse(await program.run(row));                 // …then per row
  if (outcome.ok) console.log(outcome.value, outcome.variables, outcome.steps);
  else console.error(outcome.error.code, outcome.error.message, outcome.error.path);
}
```

## API

`compile(program, executors, limits) → Program`, then
`program.run(variables) → Promise<string>`, as often as needed — or, for a
program that runs once, `run(program, variables, executors, limits) →
Promise<string>`, which compiles and runs in one go.

| Argument | |
|---|---|
| `program` | JSON text. |
| `variables` | JSON text of an object, or `""`. |
| `executors` | `{ id: { name: (call) => value, … }, … }` or `null`: each executor an object of the external functions it provides, which a program declares in `ext` (`{ "text": ["trim"] }`) and calls as `text.trim`. A call is `{ op, arg?, input, tag? }` (`op` is the function's own name, `tag` the calling node's); return a JSON value or a promise of one. `undefined` is `null`; throwing or rejecting fails the run with `function_failed`. |
| `limits` | JSON text like `{"max_steps":10000,"max_depth":64}`, or `""`. |

A run resolves with JSON text, `{"ok":true,"value":…,"variables":…,"steps":…}`
or `{"ok":false,"error":{"code":…,"message":…,"path":…,"tag"?:…,"issues"?:…}}`
for any CodyScript error — with `run`, including a program that isn't
valid or isn't JSON. It rejects only when the arguments are wrong
(variables or limits that aren't JSON, an executor that isn't an object of
functions).

`compile` throws those: a `CodyScriptError` (an `Error` with the same
`code`, `message`, `path`, `tag` and `issues`) for a program that isn't
valid or isn't JSON, and a plain `Error` for wrong arguments.

Compile a program once and keep the `Program` for every run: for a small
rule, parsing and compiling cost several times what running it does. Runs
of one `Program` don't share state and may overlap. It's freed when it's
garbage-collected, or at once with `program.free()` (or `using`).

Also exported: `version()` and `schema()` (the JSON Schema, as text).

### Debugging

`debug(program, config, executors, limits, onPause) → Promise<string>`
runs a program as `run` does, but takes a run configuration — its
variables, and its breakpoints, as JSON text (`""` for none) — and pauses
it at the breakpoints, calling `onPause(pause)` each time and waiting for
its answer, which it may give whenever it likes, from a promise:

```js
const config = '{ "variables": { "Name": " Ada " }, "breakpoints": [{ "tag": "name-required" }] }';
const outcome = JSON.parse(await debug(programJson, config, executors, "", (pause) => {
  // pause: { reason, path, kind, name?, tag?, function?, depth, steps, breakpoints,
  //          frames: [{ function?, call?, scopes: [{…}, …] }, …] }
  show(pause);                                  // your UI
  return new Promise((resolve) => (onClick = resolve));   // "step_over", …
}));
```

- A run configuration is `{ "variables"?: {…}, "breakpoints"?: [{ "tag":
  … } or { "path": … }, …] }` (its JSON Schema: `runConfigSchema()`). A
  breakpoint names the steps with a tag, or the step at a path; prefer tags,
  which edits to the program don't move. One that names no step makes the
  outcome an `invalid_config` error, listing each; `check(program, config,
  executors, limits)` says the same without running: `{"ok":true}` or
  `{"ok":false,"error":{…}}`. Text that isn't a run configuration rejects.
- It pauses only at breakpoints (`reason` `"breakpoint"`), and then where
  the actions it's given take it (`"step"`); with no breakpoints, never.
  `path` is the step's JSON Pointer, `kind` its node type
  (`"set"`, `"call"`, …), `frames` the call stack — the program's body
  first, then each CodyScript function running, each with its scopes,
  outermost first.
- `onPause` returns, or resolves to, `"step_over"`, `"step_into"`,
  `"continue"` or `"stop"` — or `{ action, breakpoints }` to replace the
  breakpoints as well (checked the same way). `"stop"` ends the run with an
  `aborted` error.
- It resolves as `run` does, and rejects as `run` does or if `onPause`
  throws, rejects or answers anything else.

`steps(program, executors, limits) → string` lists the steps breakpoints can
name, compiling the program as `run` would: `{"ok":true,"steps":[{ path,
kind, name?, tag?, function?, depth }, …]}` in the order written (the body's,
then each function's), or `{"ok":false,"error":{…}}`. See the core's
README, "Debugging", for what each action does.

## Building

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130   # exactly the wasm-bindgen crate's version
cargo build -p codyscript-wasm --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/codyscript_wasm.wasm
```

`--target web` makes an ES module for browsers; use `--target nodejs` or
`--target bundler` for those. (The playground does all this for itself.)

`npm/build.sh` builds the npm package `@cody-script/core-wasm` into
`target/npm` (the `--target web` build, with `npm/package.json` at the
crate's version and `npm/README.md`), and `node npm/smoke.mjs target/npm`
checks it in Node. CI does both; a release publishes it to GitHub Packages.

## The playground

[`codyscript-playground`](../codyscript-playground/README.md) serves a page
for smoke-testing this crate in a browser: `cargo run -p
codyscript-playground`. It's a separate crate so that its server
dependencies never become this one's.

## Tests

`tests/web.rs` runs CodyScript, compiled to WebAssembly, in a JavaScript
engine — through these bindings (JavaScript functions, sync, async and
throwing, and calls between them and CodyScript functions; the event loop
driving a run; JSON in and out; programs compiled once and run many times;
debugging with a JavaScript `onPause`) and through the core
directly (the blocking runner, signals), plus stack-depth checks under
WebAssembly's 1 MiB stack.

```bash
cargo test -p codyscript-wasm --target wasm32-unknown-unknown         # in Node
WASM_BINDGEN_USE_BROWSER=1 cargo test -p codyscript-wasm --target wasm32-unknown-unknown
                                     # in headless Chrome/Firefox (needs chromedriver/geckodriver)
WASM_BINDGEN_USE_BROWSER=1 NO_HEADLESS=1 cargo test -p codyscript-wasm --target wasm32-unknown-unknown
                                     # serves the tests at http://127.0.0.1:8000 for any browser
```

The workspace's `.cargo/config.toml` sets `wasm-bindgen-test-runner` (from
`wasm-bindgen-cli`) as the runner for the wasm32 target.

## How JavaScript executors meet the engine

The engine's executors are `Send + Sync` and return `Send` futures, so a
native host can run programs across threads. JavaScript values are neither.
Browser WebAssembly has one thread, so this crate wraps each JavaScript
function and the future of each call in `send_wrapper::SendWrapper`, which
checks at runtime that they never leave the thread that made them — which
holds by construction here. The core is unchanged and keeps its guarantees.
