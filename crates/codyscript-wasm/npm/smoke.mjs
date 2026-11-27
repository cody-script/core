// Checks a built package (build.sh's output; its directory is the argument)
// the way a Node user would load it: initialised from the module's bytes,
// then a run calling a sync and an async JavaScript function, a failing
// run, a program compiled once and run twice, a debugged run, and a
// program's tests and types.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const dir = resolve(process.argv[2] ?? "target/npm");
const pkg = JSON.parse(await readFile(join(dir, "package.json"), "utf8"));
const { default: init, compile, run, debug, steps, check, tests, test, problems, version, schema, runConfigSchema } = await import(pathToFileURL(join(dir, pkg.exports["."].default)));
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

const compiled = compile(program, executors, "");
assert.equal(JSON.parse(await compiled.run('{"Name":"  Ada "}')).value, "Ada");
assert.equal(JSON.parse(await compiled.run('{"Name":" Grace"}')).value, "Grace");
compiled.free();

assert.throws(() => compile('{ "ext": { "nope": ["x"] }, "call": { "nope.x": {} } }', null, ""), {
	name: "CodyScriptError",
	code: "unknown_function",
	path: "/ext/nope/0",
});

const listed = JSON.parse(steps(program, executors, ""));
assert.equal(listed.steps[3].tag, "name-required");
const config = JSON.stringify({ variables: { Name: "  Ada " }, breakpoints: [{ tag: "name-required" }] });
assert.equal(check(program, config, executors, ""), '{"ok":true}');
const pauses = [];
const debugged = JSON.parse(await debug(program, config, executors, "", async (pause) => {
	pauses.push(`${pause.path} (${pause.reason})`);
	return "step_over";
}));
assert.equal(debugged.value, "Ada");
assert.deepEqual(pauses, ["/do/1 (breakpoint)", "/do/2 (step)"]);
assert.equal(JSON.parse(runConfigSchema()).title, "CodyScript run configuration");

const tested = JSON.stringify({
	...JSON.parse(program),
	types: { Row: { obj: { Name: "string" } } },
	tests: { trims: { do: [{ expect: { run: { Name: " Ada " } }, eq: "Ada" }] } },
});
assert.deepEqual(JSON.parse(tests(tested, executors, "")).tests, ["trims"]);
assert.equal(JSON.parse(await test(tested, "trims", "", executors, "")).ok, true);
assert.deepEqual(JSON.parse(problems(tested, "Row", '{"Name":1}', executors, "")).problems, [
	{ path: "/Name", message: "Expected a string, got number" },
]);

console.log(`${pkg.name}@${pkg.version}: ok`);
