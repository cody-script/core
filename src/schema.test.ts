import { describe, expect, test } from "vitest";
import { createEngine } from "./engine.js";
import { CodyScriptError } from "./errors.js";
import { pointer } from "./schema.js";
import { m, math, op, ref, set, val, when } from "./test/build.js";

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
	test("accepts every node type, tagged or not", () => {
		const program = {
			...m(
				{ $: "set", set: "a", to: { $: "val", val: 1, tag: "v" }, tag: "s" },
				when(ref("a"), [], [{ $: "for", if: val(false), do: [], tag: "f" }]),
				{
					$: "ret",
					ret: op("math", "add", { a: val(1), b: val(2) }),
					tag: "r",
				},
			),
			id: "main",
			tag: "root",
		};
		expect(engine.compile(program).program).toStrictEqual(program);
	});

	test.each([
		["not an object", 5, ""],
		["unknown node type", { $: "while" }, "/$"],
		["missing field", { $: "ref" }, "/ref"],
		["typo'd field", { $: "val", val: 1, tga: "x" }, "/tga"],
		["empty variable name", m(set("", val(1))), "/do/0/set"],
		["NaN is not JSON", val(Number.NaN), "/val"],
		["Infinity is not JSON", val(Number.POSITIVE_INFINITY), "/val"],
		["undefined is not JSON", { $: "val", val: undefined }, "/val"],
		["functions are not JSON", val((() => 1) as never), "/val"],
		[
			"el must be a list",
			{ $: "m()", do: [{ $: "if", if: val(1), do: [], el: {} }] },
			"/do/0/el",
		],
	])("refuses %s", (_, program, path) => {
		const error = compileError(program);
		expect(error.code).toBe("invalid_program");
		expect(error.path).toBe(path);
		expect(error.issues?.length).toBeGreaterThan(0);
	});

	test("reports every issue, not just the first", () => {
		const error = compileError(m(set("", val(1)), { $: "ret" } as never));
		expect(error.issues?.map((i) => i.path)).toEqual([
			"/do/0/set",
			"/do/1/ret",
		]);
	});

	test("refuses a program nested past maxDepth before validating it", () => {
		let deep: unknown = val(1);
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

	test("refuses an unknown executor at compile time, wherever it is", () => {
		const cases: [unknown, string][] = [
			[op("nope", "x"), "/exe/id"],
			[op("math", "add", { a: op("nope", "x") }), "/op/map/a/exe/id"],
			[m(set("x", op("nope", "x"))), "/do/0/to/exe/id"],
			[m({ $: "ret", ret: op("nope", "x") }), "/do/0/ret/exe/id"],
			[m(when(op("nope", "x"), [])), "/do/0/if/exe/id"],
			[m(when(val(1), [set("x", op("nope", "x"))])), "/do/0/do/0/to/exe/id"],
			[
				m(when(val(1), [], [set("x", op("nope", "x"))])),
				"/do/0/el/0/to/exe/id",
			],
			[m({ $: "for", if: op("nope", "x"), do: [] }), "/do/0/if/exe/id"],
			[
				m({ $: "for", if: val(0), do: [set("x", op("nope", "x"))] }),
				"/do/0/do/0/to/exe/id",
			],
			[m(set("x", m(set("y", op("nope", "x"))))), "/do/0/to/do/0/to/exe/id"],
		];
		for (const [program, path] of cases) {
			const error = compileError(program);
			expect(error.code).toBe("unknown_executor");
			expect(error.path).toBe(path);
		}
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

	test("a variable named after an Object.prototype member is just a variable", async () => {
		const error = await createEngine()
			.run(ref("constructor"))
			.catch((e: CodyScriptError) => e);
		expect((error as CodyScriptError).code).toBe("undefined_variable");
		const { variables } = await createEngine().run(m(set("__proto__", val(1))));
		expect(Object.getPrototypeOf(variables)).toBe(Object.prototype);
		expect(Object.keys(variables)).toEqual(["__proto__"]);
	});

	test.each(["__proto__", "prototype", "constructor"])(
		"refuses the reserved key %s anywhere rather than dropping it",
		(key) => {
			const programs = [
				JSON.parse(`{"$":"val","val":{"${key}":1}}`),
				JSON.parse(
					`{"$":"op","op":{"id":"add","map":{"${key}":{"$":"val","val":1}}},"exe":{"id":"math"}}`,
				),
			];
			for (const program of programs) {
				const error = compileError(program);
				expect(error.code).toBe("invalid_program");
				expect(error.message).toContain(`"${key}" is a reserved key`);
			}
		},
	);

	test("pointer escapes ~ and /", () => {
		expect(pointer(["a/b", "c~d", 0])).toBe("/a~1b/c~0d/0");
	});
});
