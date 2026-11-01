import { describe, expect, test } from "vitest";
import { createEngine } from "./engine.js";
import { CodyScriptError } from "./errors.js";
import { m, math, op, ref, ret, set, val, when } from "./test/build.js";
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
		["a missing $", { val: 1 }, "/$"],
		["an instruction that isn't an object", m("set" as never), "/do/0"],
		["an input where an instruction belongs", m(val(1) as never), "/do/0/$"],
		["a non-string tag", { $: "val", val: 1, tag: 1 }, "/tag"],
		["a non-string method id", { $: "m()", do: [], id: 1 }, "/id"],
		[
			"a non-string op id",
			{ $: "op", op: { id: 1 }, exe: { id: "math" } },
			"/op/id",
		],
		[
			"op that isn't an object",
			{ $: "op", op: "add", exe: { id: "math" } },
			"/op",
		],
		[
			"a map that is a list",
			{ $: "op", op: { id: "add", map: [] }, exe: { id: "math" } },
			"/op/map",
		],
		["a bad map argument", op("math", "add", { a: 1 as never }), "/op/map/a"],
		[
			"an unknown op field",
			{ $: "op", op: { id: "add", args: {} }, exe: { id: "math" } },
			"/op/args",
		],
		[
			"an unknown exe field",
			{ $: "op", op: { id: "add" }, exe: { id: "math", fn: 1 } },
			"/exe/fn",
		],
		["missing exe", { $: "op", op: { id: "add" } }, "/exe"],
		[
			"a non-JSON exe arg",
			op("math", "add", undefined, new Date() as never),
			"/exe/arg",
		],
		["a Date is not JSON", val(new Date() as never), "/val"],
		["a class instance is not JSON", val(new (class {})() as never), "/val"],
		["a nested non-JSON value", val({ a: [1, Number.NaN] }), "/val/a/1"],
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

	test("an unknown executor carries its node's tag", () => {
		const error = compileError({ ...op("nope", "x"), tag: "n7" });
		expect(error.tag).toBe("n7");
		expect(error.message).toBe('Unknown executor "nope" at /exe/id (tag n7)');
	});

	test("shape problems and unknown executors are reported together", () => {
		const error = compileError(m(set("x", op("nope", "x")), set("", val(1))));
		expect(error.code).toBe("unknown_executor");
		expect(error.issues?.map((i) => i.path)).toEqual([
			"/do/0/to/exe/id",
			"/do/1/set",
		]);
	});

	test("objects with no prototype are JSON too", () => {
		const bare = Object.assign(Object.create(null), { a: 1 });
		expect(() => engine.compile(val(bare))).not.toThrow();
	});

	test("changing the source after compile doesn't change the program", async () => {
		const source = m(ret(val(1))) as { do: { ret: { val: unknown } }[] };
		const compiled = engine.compile(source);
		source.do[0]!.ret.val = () => "smuggled";
		expect((await compiled.run()).value).toBe(1);
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
