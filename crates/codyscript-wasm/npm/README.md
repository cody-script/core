# @cody-script/core-wasm

[CodyScript](https://github.com/cody-script/core) compiled to WebAssembly:
run programs written as JSON from JavaScript, in a browser or Node, with
functions written in JavaScript — plain or `async`. Hosts supply the
external functions; programs supply the logic.

```bash
# From GitHub Packages: map the scope once, in .npmrc
echo "@cody-script:registry=https://npm.pkg.github.com" >> .npmrc
npm install @cody-script/core-wasm
```

```js
import init, { compile, run } from "@cody-script/core-wasm";
await init(); // in Node: await init({ module_or_path: await readFile(wasmPath) })

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

In Node, `init()` can't fetch the module from a file URL, so pass its bytes:

```js
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
const wasmPath = createRequire(import.meta.url).resolve("@cody-script/core-wasm/codyscript_wasm_bg.wasm");
await init({ module_or_path: await readFile(wasmPath) });
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
its answer, which may come from a promise — when a person clicks, say:

```js
const config = '{ "variables": { "Name": " Ada " }, "breakpoints": [{ "tag": "name-required" }] }';
const outcome = JSON.parse(await debug(programJson, config, executors, "", (pause) => {
  // pause: { reason, path, kind, name?, tag?, function?, depth, steps, breakpoints,
  //          frames: [{ function?, call?, scopes: [{…}, …] }, …] }
  show(pause);                                  // your UI
  return new Promise((resolve) => (onClick = resolve));   // "step_over", …
}));
```

A run configuration is `{ "variables"?: {…}, "breakpoints"?: [{ "tag": … }
or { "path": … }, …] }`; `runConfigSchema()` is its JSON Schema. A
breakpoint names the steps with a tag, or the step at a path, as
`steps(program, executors, limits)` lists them —
`{"ok":true,"steps":[{ path, kind, name?, tag?, function?, depth }, …]}`;
prefer tags, which edits to the program don't move. A breakpoint that names
no step makes the outcome an `invalid_config` error; `check(program, config,
executors, limits)` says so without running (`{"ok":true}` or
`{"ok":false,"error":{…}}`).

It pauses only at breakpoints (`reason: "breakpoint"`), and then where the
actions it's given take it (`"step"`); with no breakpoints, never. Answer
`"step_over"`, `"step_into"`, `"continue"` or `"stop"` (which ends the run
with an `aborted` error), or `{ action, breakpoints }` to replace the
breakpoints too. It resolves as `run` does, and also rejects if `config`
isn't a run configuration, or `onPause` throws or answers anything else.

The language itself — nodes, expressions, errors and limits — is documented
in the [core's README](https://github.com/cody-script/core/tree/main/crates/codyscript-core#readme).
