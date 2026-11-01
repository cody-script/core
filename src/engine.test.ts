import { describe, expect, test, vi } from "vitest";
import { createEngine } from "./engine.js";
import { CodyScriptError } from "./errors.js";
import { loop, m, math, op, ref, ret, set, val, when } from "./test/build.js";
import type { Executor, Value } from "./types.js";

const engine = createEngine({ executors: { math } });

async function failure(promise: Promise<unknown>): Promise<CodyScriptError> {
	const error = await promise.then(
		() => undefined,
		(e: unknown) => e,
	);
	expect(error).toBeInstanceOf(CodyScriptError);
	return error as CodyScriptError;
}

describe("inputs", () => {
	test("a constant is its value", async () => {
		for (const v of [
			0,
			"s",
			true,
			null,
			[1, [2]],
			{ a: { b: 1 } },
		] as Value[]) {
			expect((await engine.run(val(v))).value).toStrictEqual(v);
		}
	});

	test("a reference reads a variable", async () => {
		expect((await engine.run(ref("x"), { variables: { x: 7 } })).value).toBe(7);
		expect(
			(await engine.run(ref("x"), { variables: { x: null } })).value,
		).toBeNull();
	});

	test("an undefined reference fails rather than yielding undefined", async () => {
		const error = await failure(
			engine.run({ $: "ref", ref: "nope", tag: "t1" }),
		);
		expect(error.code).toBe("undefined_variable");
		expect(error.tag).toBe("t1");
		expect(error.message).toBe('Variable "nope" is not defined (tag t1)');
	});

	test("a method with no return is null", async () => {
		expect((await engine.run(m())).value).toBeNull();
	});
});

describe("operations", () => {
	test("call the executor with op, arg and resolved input", async () => {
		const exe = vi.fn<Executor>(() => "ok");
		const e = createEngine({ executors: { exe } });
		const result = await e.run(
			op("exe", "doIt", { a: val(1), b: ref("x") }, { cfg: true }),
			{
				variables: { x: "y" },
			},
		);
		expect(result.value).toBe("ok");
		expect(exe).toHaveBeenCalledWith({
			op: "doIt",
			arg: { cfg: true },
			input: { a: 1, b: "y" },
		});
	});

	test("omit input and arg when the program has none", async () => {
		const exe = vi.fn<Executor>(() => 1);
		await createEngine({ executors: { exe } }).run(op("exe", "x"));
		expect(exe).toHaveBeenCalledWith({ op: "x" });
	});

	test("undefined from an executor is null", async () => {
		const e = createEngine({ executors: { exe: () => undefined } });
		expect((await e.run(op("exe", "x"))).value).toBeNull();
	});

	test("async executors are awaited", async () => {
		const e = createEngine({ executors: { exe: async () => 5 } });
		expect((await e.run(op("exe", "x"))).value).toBe(5);
	});

	test("arguments resolve one at a time, in order", async () => {
		const log: string[] = [];
		const e = createEngine({
			executors: {
				slow: async ({ arg }) => {
					await new Promise((r) => setTimeout(r, arg === "a" ? 10 : 0));
					log.push(arg as string);
					return arg ?? null;
				},
				pair: ({ input }) => input ?? null,
			},
		});
		const result = await e.run(
			op("pair", "x", {
				first: op("slow", "x", undefined, "a"),
				second: op("slow", "x", undefined, "b"),
			}),
		);
		expect(log).toEqual(["a", "b"]);
		expect(result.value).toEqual({ first: "a", second: "b" });
	});

	test("an executor's error fails the run with its path and cause", async () => {
		const boom = new Error("boom");
		const e = createEngine({
			executors: {
				bad: () => {
					throw boom;
				},
			},
		});
		const error = await failure(
			e.run(m(set("x", { ...op("bad", "go"), tag: "n1" }))),
		);
		expect(error.code).toBe("executor_failed");
		expect(error.path).toBe("/do/0/to");
		expect(error.cause).toBe(boom);
		expect(error.message).toBe(
			'Executor "bad" failed on "go": boom at /do/0/to (tag n1)',
		);
	});

	test("a non-Error throw is reported too", async () => {
		const e = createEngine({
			executors: { bad: () => Promise.reject("nope") },
		});
		const error = await failure(e.run(op("bad", "go")));
		expect(error.message).toContain(": nope");
	});
});

describe("instructions", () => {
	test("set assigns and the result reports top-level variables", async () => {
		const result = await engine.run(m(set("a", val(1)), set("b", ref("a"))), {
			variables: { c: 3 },
		});
		expect(result.variables).toEqual({ a: 1, b: 1, c: 3 });
		expect(result.value).toBeNull();
	});

	test("ret stops the method", async () => {
		const result = await engine.run(m(ret(val(1)), set("never", val(2))));
		expect(result).toMatchObject({ value: 1, variables: {} });
	});

	test("if runs do or el", async () => {
		const program = (c: Value) =>
			m(when(val(c), [ret(val("yes"))], [ret(val("no"))]));
		expect((await engine.run(program(true))).value).toBe("yes");
		expect((await engine.run(program(0))).value).toBe("no");
		expect((await engine.run(program([]))).value).toBe("yes");
		expect(
			(await engine.run(m(when(val(false), [ret(val(1))])))).value,
		).toBeNull();
	});

	test("ret inside if leaves the method, skipping the rest", async () => {
		const program = m(
			when(val(true), [ret(val(1)), set("x", val(2))]),
			set("y", val(3)),
		);
		expect(await engine.run(program)).toMatchObject({
			value: 1,
			variables: {},
		});
	});

	test("for loops while its condition holds", async () => {
		const program = m(
			set("i", val(0)),
			loop(op("math", "lt", { a: ref("i"), b: val(5) }), [
				set("i", op("math", "add", { a: ref("i"), b: val(1) })),
			]),
			ret(ref("i")),
		);
		expect((await engine.run(program)).value).toBe(5);
	});

	test("ret leaves an endless loop", async () => {
		expect((await engine.run(m(loop(val(true), [ret(val(45))])))).value).toBe(
			45,
		);
	});
});

describe("methods", () => {
	test("a nested method returns to its caller, which carries on", async () => {
		// The 0.0.x engine ran nested methods in the caller's frame, so this
		// returned 1 and never set y.
		const program = m(
			set("x", m(ret(val(1)))),
			set("y", val(2)),
			ret(val("end")),
		);
		expect(await engine.run(program)).toMatchObject({
			value: "end",
			variables: { x: 1, y: 2 },
		});
	});

	test("a nested method reads outer variables but assigns its own", async () => {
		const program = m(
			set("a", val(1)),
			set("b", m(set("a", val(99)), ret(ref("a")))),
			ret(ref("a")),
		);
		expect(await engine.run(program)).toMatchObject({
			value: 1,
			variables: { a: 1, b: 99 },
		});
	});

	test("a nested method's variables don't leak out", async () => {
		const error = await failure(
			engine.run(m(set("_", m(set("inner", val(1)))), ret(ref("inner")))),
		);
		expect(error.code).toBe("undefined_variable");
		expect(error.path).toBe("/do/1/ret");
	});
});

describe("compile", () => {
	test("a compiled program runs many times with fresh state", async () => {
		const compiled = engine.compile(
			m(set("x", op("math", "add", { a: ref("n"), b: val(1) })), ret(ref("x"))),
		);
		expect((await compiled.run({ variables: { n: 1 } })).value).toBe(2);
		expect((await compiled.run({ variables: { n: 10 } })).value).toBe(11);
		expect(compiled.program.$).toBe("m()");
	});

	test("input variables are copied, not mutated", async () => {
		const variables = { a: 1 };
		await engine.run(m(set("a", val(2))), { variables });
		expect(variables).toEqual({ a: 1 });
	});

	test("engines don't share executors", async () => {
		createEngine({ executors: { other: () => 1 } });
		expect(() => engine.compile(op("other", "x"))).toThrow(CodyScriptError);
	});

	test("executors may be given as a Map", async () => {
		const e = createEngine({ executors: new Map([["one", () => 1]]) });
		expect((await e.run(op("one", "x"))).value).toBe(1);
	});

	test("an engine with no executors runs pure programs", async () => {
		expect((await createEngine().run(val(1))).value).toBe(1);
	});
});

describe("limits", () => {
	test("an endless loop hits the step limit", async () => {
		const e = createEngine({ limits: { maxSteps: 50 } });
		const error = await failure(
			e.run(m({ $: "for", if: val(true), do: [], tag: "spin" })),
		);
		expect(error.code).toBe("step_limit");
		expect(error.path).toBe("/do/0/if");
		expect(error.message).toBe("Program exceeded 50 steps at /do/0/if");
	});

	test("the default limit stops an endless loop too", async () => {
		const error = await failure(createEngine().run(m(loop(val(true), []))));
		expect(error.code).toBe("step_limit");
	});

	test("steps are reported", async () => {
		// method body: set (1) + its val (1)
		expect((await engine.run(m(set("a", val(1))))).steps).toBe(2);
	});

	test("an aborted signal stops the run and reaches executors", async () => {
		const controller = new AbortController();
		const seen: unknown[] = [];
		const e = createEngine({
			executors: {
				stop: ({ signal }) => {
					seen.push(signal);
					controller.abort("enough");
					return true;
				},
			},
		});
		const error = await failure(
			e.run(m(loop(op("stop", "x"), [])), { signal: controller.signal }),
		);
		expect(seen).toEqual([controller.signal]);
		expect(error.code).toBe("aborted");
		expect(error.cause).toBe("enough");
	});

	test("an executor's own CodyScriptError passes through unwrapped", async () => {
		const own = new CodyScriptError("aborted", "stop");
		const e = createEngine({
			executors: {
				x: () => {
					throw own;
				},
			},
		});
		expect(await failure(e.run(op("x", "y")))).toBe(own);
	});
});
