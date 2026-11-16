# @cody-script/core-wasm

[CodyScript](https://github.com/cody-script/core) compiled to WebAssembly:
run programs written as JSON from JavaScript, in a browser or Node, with
executors written in JavaScript — plain functions or `async` ones. Hosts
supply the operations; programs supply the logic.

```bash
# From GitHub Packages: map the scope once, in .npmrc
echo "@cody-script:registry=https://npm.pkg.github.com" >> .npmrc
npm install @cody-script/core-wasm
```

```js
import init, { run } from "@cody-script/core-wasm";
await init(); // in Node: await init({ module_or_path: await readFile(wasmPath) })

const executors = {
  text: ({ op, input }) => (op === "trim" ? String(input.value ?? "").trim() : null),
  lookup: async ({ input }) => (await fetch(`/api/${input.id}`)).json(),
};
const outcome = JSON.parse(await run(programJson, '{"Name":"  Ada "}', executors, '{"maxSteps":10000}'));
if (outcome.ok) console.log(outcome.value, outcome.variables, outcome.steps);
else console.error(outcome.error.code, outcome.error.message, outcome.error.path);
```

In Node, `init()` can't fetch the module from a file URL, so pass its bytes:

```js
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
const wasmPath = createRequire(import.meta.url).resolve("@cody-script/core-wasm/codyscript_wasm_bg.wasm");
await init({ module_or_path: await readFile(wasmPath) });
```

## API

`run(program, variables, executors, limits) → Promise<string>`

| Argument | |
|---|---|
| `program` | JSON text. |
| `variables` | JSON text of an object, or `""`. |
| `executors` | `{ id: (call) => value }` or `null`. A call is `{ op, arg?, input }`; return a JSON value or a promise of one. `undefined` is `null`; throwing or rejecting fails the run with `executor_failed`. |
| `limits` | JSON text like `{"maxSteps":10000,"maxDepth":64}`, or `""`. |

It resolves with JSON text, `{"ok":true,"value":…,"variables":…,"steps":…}`
or `{"ok":false,"error":{"code":…,"message":…,"path":…,"tag"?:…,"issues"?:…}}`
for any CodyScript error, including a program that isn't JSON. It rejects
only when the arguments are wrong (variables or limits that aren't JSON, an
executor that isn't a function). Also exported: `version()` and `schema()`
(the JSON Schema, as text).

The language itself — nodes, expressions, errors and limits — is documented
in the [core's README](https://github.com/cody-script/core/tree/main/crates/codyscript-core#readme).
