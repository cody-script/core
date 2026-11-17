// Checks a built package (build.sh's output; its directory is the argument)
// the way a Node user would load it: initialised from the module's bytes,
// then a run calling a sync and an async JavaScript function, and a failing run.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const dir = resolve(process.argv[2] ?? "target/npm");
const pkg = JSON.parse(await readFile(join(dir, "package.json"), "utf8"));
const { default: init, run, version, schema } = await import(pathToFileURL(join(dir, pkg.exports["."].default)));
await init({ module_or_path: await readFile(join(dir, "codyscript_wasm_bg.wasm")) });

assert.equal(version(), pkg.version);
assert.equal(typeof JSON.parse(schema()), "object");

const program = JSON.stringify({
	ext: { text: ["trim", "empty"] },
	do: [
		{ set: { name: { call: { "text.trim": { value: { ref: "Name" } } } } } },
		{
			if: { call: { "text.empty": { value: { ref: "name" } } } },
			do: [{ ret: { val: { error: "Name is required" } } }],
			tag: "name-required",
		},
		{ ret: { ref: "name" } },
	],
});
const executors = {
	text: {
		trim: ({ input }) => String(input.value ?? "").trim(),
		empty: async ({ input }) => input.value === "",
	},
};

const ok = JSON.parse(await run(program, '{"Name":"  Ada "}', executors, ""));
assert.equal(ok.ok, true);
assert.equal(ok.value, "Ada");

const empty = JSON.parse(await run(program, '{"Name":"   "}', executors, ""));
assert.deepEqual(empty.value, { error: "Name is required" });

const failed = JSON.parse(await run("{", "", null, ""));
assert.equal(failed.ok, false);

console.log(`${pkg.name}@${pkg.version}: ok`);
