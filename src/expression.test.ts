import { describe, expect, test } from "vitest";
import { createEngine } from "./engine.js";
import { CodyScriptError } from "./errors.js";
import { m, ret, set } from "./test/build.js";
import type { Value } from "./types.js";

const engine = createEngine();
const x = (source: string, tag?: string) => ({
	x: source,
	...(tag && { tag }),
});
const run = async (source: string, variables: { [k: string]: Value } = {}) =>
	(await engine.run(x(source), { variables })).value;

async function failure(promise: Promise<unknown>): Promise<CodyScriptError> {
	const error = await promise.then(
		() => undefined,
		(e: unknown) => e,
	);
	expect(error).toBeInstanceOf(CodyScriptError);
	return error as CodyScriptError;
}

function compileError(source: string, maxDepth?: number): CodyScriptError {
	try {
		createEngine({ limits: { maxDepth } }).compile(x(source));
	} catch (error) {
		return error as CodyScriptError;
	}
	throw new Error("expected compile to fail");
}

describe("expressions", () => {
	test.each([
		["1 + 2 * 3", 7],
		["(1 + 2) * 3", 9],
		["10 / 4", 2.5],
		["2 - -1", 3],
		["10 - 4 - 3", 3],
		["-a + 1", -1],
		["a * 0.03", 0.06],
		["gross_sales - discounts - promotions", 75],
	])("%s is %s", async (source, value) => {
		const vars = { a: 2, gross_sales: 100, discounts: 20, promotions: 5 };
		expect(await run(source, vars)).toBe(value);
	});

	test.each([
		["a < b", true],
		["a <= 1", true],
		["a > b", false],
		["b >= 2", true],
		["a == 1", true],
		["a != 1", false],
		["'apple' < 'banana'", true],
		["s == 'paid'", true],
		["n == null", true],
		["true == true", true],
		["1 == '1'", false],
		["a < b && b < 3", true],
	])("%s is %s", async (source, value) => {
		expect(await run(source, { a: 1, b: 2, s: "paid", n: null })).toBe(value);
	});

	test.each([
		["a && b", 2],
		["0 && b", 0],
		["a || b", 1],
		["n || 'default'", "default"],
		["!a", false],
		["!n", true],
		["!!s", true],
	])("%s is %s (&& and || return an operand)", async (source, value) => {
		expect(await run(source, { a: 1, b: 2, s: "x", n: null })).toBe(value);
	});

	test("&& and || short-circuit", async () => {
		expect(await run("false && missing")).toBe(false);
		expect(await run("true || missing")).toBe(true);
	});

	test("literals: text with escapes, true, false, null", async () => {
		expect(await run("'it\\'s'")).toBe("it's");
		expect(await run("''")).toBe("");
		expect(await run("null")).toBeNull();
		expect(await run("  1  ")).toBe(1);
	});

	test("reads the enclosing scope and works anywhere an input does", async () => {
		const program = m(
			set("i", 0),
			{ for: x("i < 3"), do: [set("i", x("i + 1"))] },
			{ if: x("i == 3 && !done"), do: [ret(m(ret(x("i * 10"))))], el: [] },
		);
		expect(
			(await engine.run(program, { variables: { done: false } })).value,
		).toBe(30);
	});

	test("a compiled expression runs many times", async () => {
		const compiled = engine.compile(x("a * 2"));
		expect((await compiled.run({ variables: { a: 2 } })).value).toBe(4);
		expect((await compiled.run({ variables: { a: 5 } })).value).toBe(10);
	});

	test("an undefined variable fails at its expression, with the source", async () => {
		const error = await failure(
			engine.run(m(set("fee", x("amount * rate", "fee")))),
		);
		expect(error.code).toBe("undefined_variable");
		expect(error.path).toBe("/do/0/set/fee/x");
		expect(error.tag).toBe("fee");
		expect(error.message).toBe(
			'Variable "amount" is not defined in "amount * rate" at /do/0/set/fee/x (tag fee)',
		);
	});

	test.each([
		["1 + 'a'", '"+" needs numbers, got number and string'],
		["-s", '"-" needs a number, got string'],
		["'a' < 1", '"<" needs two numbers or two texts, got string and number'],
		["n >= 1", '">=" needs two numbers or two texts, got null and number'],
		["l == 1", '"==" can\'t compare list and number'],
		["1 != o", '"!=" can\'t compare number and object'],
		["1 / 0", '"/" gave Infinity (division by zero?)'],
		["0 / 0", '"/" gave NaN (division by zero?)'],
	])("%s is a type error", async (source, message) => {
		const vars = { s: "x", n: null, l: [1], o: { a: 1 } };
		const error = await failure(engine.run(x(source), { variables: vars }));
		expect(error.code).toBe("type_error");
		expect(error.path).toBe("/x");
		expect(error.message).toBe(`${message} in "${source}" at /x`);
	});

	test("every operator counts toward maxSteps", async () => {
		const limited = createEngine({ limits: { maxSteps: 5 } });
		// The node (1), then 1 + 2 * 3: +, 1, *, 2, 3 (5 more).
		const error = await failure(limited.run(x("1 + 2 * 3")));
		expect(error.code).toBe("step_limit");
		expect(error.path).toBe("/x");
		expect((await createEngine().run(x("1 + 2 * 3"))).steps).toBe(6);
	});

	test.each([
		["1 +", "Expression ends too early"],
		["", "Expected a non-empty string"],
		["a b", 'Unexpected "b"'],
		["1 2", "Unexpected 2"],
		["(1 + 2", 'Missing ")"'],
		["1 + 2)", 'Unexpected ")"'],
		["1 < 2 < 3", "Comparisons can't be chained; use &&"],
		["a = 1", 'Unexpected "="'],
		["a @ b", 'Unexpected "@"'],
		["'open", 'Unexpected "\'"'],
		["*", 'Unexpected "*"'],
	])("refuses %j at compile time", (source, message) => {
		const error = compileError(source);
		expect(error.code).toBe("invalid_program");
		expect(error.path).toBe("/x");
		expect(error.message).toContain(message);
	});

	test("refuses nesting past maxDepth, parentheses or operators", () => {
		expect(
			compileError(`${"(".repeat(200)}1${")".repeat(200)}`).message,
		).toContain("Expression nests deeper than 128 levels");
		expect(compileError(`${"!".repeat(200)}a`).message).toContain(
			"nests deeper",
		);
		expect(compileError("((1))", 2).message).toContain("nests deeper than 2");
	});

	test("refuses a non-string expression", () => {
		const error = (() => {
			try {
				engine.compile({ x: 1 });
			} catch (e) {
				return e as CodyScriptError;
			}
		})();
		expect(error?.path).toBe("/x");
	});
});
