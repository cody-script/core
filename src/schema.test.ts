import { readFileSync } from "node:fs";
import { Ajv2020 } from "ajv/dist/2020.js";
import { describe, expect, test } from "vitest";
import { invalid, valid } from "./test/programs.js";

// schema.json is what editors check hand-written programs against; it must
// accept and refuse the same programs as the engine's own validator.
const schema = JSON.parse(
	readFileSync(new URL("../schema.json", import.meta.url), "utf8"),
);
const check = new Ajv2020({
	strict: true,
	allowUnionTypes: true,
	allErrors: true,
}).compile(schema);

describe("schema.json", () => {
	test.each(valid)("accepts %s", (_, program) => {
		expect(check(program), JSON.stringify(check.errors)).toBe(true);
	});

	test.each(invalid)("refuses %s", (_, program) => {
		expect(check(program)).toBe(false);
	});

	test("accepts every program in the README", () => {
		const readme = readFileSync(
			new URL("../README.md", import.meta.url),
			"utf8",
		);
		const programs = [...readme.matchAll(/```jsonc?\n([\s\S]*?)```/g)]
			.map((block) => JSON.parse(block[1]!))
			.filter((json) => typeof json === "object" && "do" in json);
		expect(programs).toHaveLength(2);
		for (const program of programs)
			expect(check(program), JSON.stringify(check.errors)).toBe(true);
	});
});
