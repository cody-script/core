import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { CodyScriptError, type Executor, createEngine } from "./index.js";

// The README's example, run as written, so the docs can't drift from the code.
const readme = readFileSync(new URL("../README.md", import.meta.url), "utf8");
const rule = JSON.parse(readme.match(/```json\n([\s\S]*?)```/)![1]!);

const text: Executor = ({ op, input }) => {
	const value = String(input?.value ?? "");
	switch (op) {
		case "trim":
			return value.trim();
		case "empty":
			return value.length === 0;
		default:
			throw new Error(`text has no "${op}"`);
	}
};

test("the README example works", async () => {
	const compiled = createEngine({ executors: { text } }).compile(rule);
	expect((await compiled.run({ variables: { Name: "  Ada " } })).value).toBe(
		"Ada",
	);
	expect((await compiled.run({ variables: { Name: "  " } })).value).toEqual({
		error: "Name is required",
	});
	await expect(compiled.run()).rejects.toThrow(CodyScriptError);
});
