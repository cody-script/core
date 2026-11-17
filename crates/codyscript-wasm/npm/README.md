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
const program = compile(programJson, executors, '{"maxSteps":10000}'); // once…
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
| `executors` | `{ id: { name: (call) => value, … }, … }` or `null`: each executor an object of the external functions it provides, which a program declares in `ext` (`{ "text": ["trim"] }`) and calls as `text.trim`. A call is `{ op, arg?, input }` (`op` is the function's own name); return a JSON value or a promise of one. `undefined` is `null`; throwing or rejecting fails the run with `function_failed`. |
| `limits` | JSON text like `{"maxSteps":10000,"maxDepth":64}`, or `""`. |

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

The language itself — nodes, expressions, errors and limits — is documented
in the [core's README](https://github.com/cody-script/core/tree/main/crates/codyscript-core#readme).
