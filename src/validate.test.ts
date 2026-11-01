import { describe, expect, test } from "vitest";
import { createEngine } from "./engine.js";
import { CodyScriptError } from "./errors.js";
import { m, math, op, ref, ret, set, val, when } from "./test/build.js";
import { invalid, valid } from "./test/programs.js";
import { pointer } from "./validate.js";

const engine = createEngine({ executors: { math } });

function compileError(program: unknown, e = engine): CodyScriptError {
	try {
		e.compile(program);
	} catch (error) {
		expect(error).toBeInstanceOf(CodyScriptError);
		return error as CodyScriptError;
	}
	throw new Error("expected compile to fail");
}

describe("validation", () => {
	test.each(valid)("accepts %s", (_, program) => {
		expect(engine.compile(program).program).toStrictEqual(program);
	});

	test.each(invalid)("refuses %s", (_, program, path) => {
		const error = compileError(program);
		expect(error.code).toBe("invalid_program");
		expect(error.path).toBe(path);
		expect(error.issues?.length).toBeGreaterThan(0);
	});

	// Not expressible in JSON, so not in the shared fixtures.
	test.each([
		["NaN", Number.NaN, ""],
		["Infinity", val(Number.POSITIVE_INFINITY), "/val"],
		["undefined", { $: "val", val: undefined }, "/val"],
		["a function", val((() => 1) as never), "/val"],
		["a Date", val(new Date() as never), "/val"],
		["a class instance", val(new (class {})() as never), "/val"],
		["a nested NaN", val({ a: [1, Number.NaN] }), "/val/a/1"],
		[
			"a non-JSON arg",
			op("math", "add", undefined, new Date() as never),
			"/arg",
		],
	])("refuses %s, which is not JSON", (_, program, path) => {
		const error = compileError(program);
		expect(error.code).toBe("invalid_program");
		expect(error.path).toBe(path);
	});

	test("objects with no prototype are JSON too", () => {
		const bare = Object.assign(Object.create(null), { a: 1 });
		expect(() => engine.compile(val(bare))).not.toThrow();
	});

	test("reports every issue, not just the first", () => {
		const error = compileError(m(set("", 1), { $: "ret" } as never));
		expect(error.issues?.map((i) => i.path)).toEqual([
			"/do/0/set",
			"/do/1/ret",
		]);
	});

	test("refuses an unknown executor wherever it is, with its node's tag", () => {
		const nope = op("nope", "x");
		const cases: [unknown, string][] = [
			[{ ...nope, tag: "n7" }, "/op"],
			[op("math", "add", { a: nope }), "/map/a/op"],
			[m(set("x", nope)), "/do/0/to/op"],
			[m(ret(nope)), "/do/0/ret/op"],
			[m(when(nope, [])), "/do/0/if/op"],
			[m(when(1, [set("x", nope)])), "/do/0/do/0/to/op"],
			[m(when(1, [], [set("x", nope)])), "/do/0/el/0/to/op"],
			[m({ $: "for", if: nope, do: [] }), "/do/0/if/op"],
			[m({ $: "for", if: 0, do: [set("x", nope)] }), "/do/0/do/0/to/op"],
			[m(set("x", m(set("y", nope)))), "/do/0/to/do/0/to/op"],
		];
		for (const [program, path] of cases) {
			const error = compileError(program);
			expect(error.code).toBe("unknown_executor");
			expect(error.path).toBe(path);
		}
		expect(compileError(cases[0]![0]).message).toBe(
			'Unknown executor "nope" at /op (tag n7)',
		);
	});

	test("shape problems and unknown executors are reported together", () => {
		const error = compileError(m(set("x", op("nope", "x")), set("", 1)));
		expect(error.code).toBe("unknown_executor");
		expect(error.issues?.map((i) => i.path)).toEqual([
			"/do/0/to/op",
			"/do/1/set",
		]);
	});

	test("executor ids are looked up as own keys only", () => {
		for (const id of [
			"constructor",
			"toString",
			"__proto__",
			"hasOwnProperty",
		]) {
			expect(compileError(op(id, "x")).code).toBe("unknown_executor");
		}
	});

	test("changing the source after compile doesn't change the program", async () => {
		const source = m(ret(val(1))) as { do: { ret: { val: unknown } }[] };
		const compiled = engine.compile(source);
		source.do[0]!.ret.val = () => "smuggled";
		expect((await compiled.run()).value).toBe(1);
	});

	test("refuses a program nested past maxDepth before validating it", () => {
		let deep: unknown = 1;
		for (let i = 0; i < 100_000; i++) deep = m(set("x", deep as never));
		const error = compileError(deep);
		expect(error.code).toBe("invalid_program");
		expect(error.message).toMatch(/deeper than 128 levels/);
	});

	test("maxDepth is configurable", () => {
		const shallow = createEngine({ limits: { maxDepth: 3 } });
		expect(() => shallow.compile(val({ a: 1 }))).not.toThrow();
		expect(compileError(val({ a: { b: { c: 1 } } }), shallow).path).toBe(
			"/val/a/b",
		);
	});

	test("a variable named after an Object.prototype member is just a variable", async () => {
		const error = await createEngine()
			.run(ref("constructor"))
			.catch((e: CodyScriptError) => e);
		expect((error as CodyScriptError).code).toBe("undefined_variable");
		const { variables } = await createEngine().run(m(set("__proto__", 1)));
		expect(Object.getPrototypeOf(variables)).toBe(Object.prototype);
		expect(Object.keys(variables)).toEqual(["__proto__"]);
	});

	test("pointer escapes ~ and /", () => {
		expect(pointer(["a/b", "c~d", 0])).toBe("/a~1b/c~0d/0");
	});
});
