import { loop, m, op, ref, set, val, when } from "./build.js";

/**
 * Programs both the engine's validator and `schema.json` must accept (given a
 * `math` executor) — shared so the two can't drift apart.
 */
export const valid: [string, unknown][] = [
	["a bare number", 5],
	["a bare string", "text"],
	["a bare boolean", false],
	["null", null],
	["a constant object", val({ a: [1, "x", null] })],
	["a constant object whose keys look like nodes", val({ ref: "x", op: {} })],
	["a reference", ref("x")],
	["a call with no arguments", op("math", "now")],
	[
		"a call with arguments and arg",
		op("math", "add", { a: 1, b: ref("x") }, { round: true }),
	],
	["an operation whose name has dots", op("math", "round.half.up")],
	["an empty block", m()],
	["a loop", m(loop(true, [{ ret: 1 }]))],
	[
		"every node type, tagged",
		{
			...m(
				{ set: { a: { val: 1, tag: "v" } }, tag: "s" },
				{ ...when(ref("a"), [], [{ for: false, do: [], tag: "f" }]), tag: "i" },
				{ ret: { ...op("math", "add", { a: 1, b: 2 }), tag: "o" }, tag: "r" },
			),
			tag: "root",
		},
	],
];

/** Programs both must refuse, with the path the validator reports first. */
export const invalid: [string, unknown, string][] = [
	["an input with no type key", { rf: "x" }, ""],
	["an input with two type keys", { ref: "a", val: 1 }, ""],
	["an instruction in an input position", { if: 1, do: [] }, "/if"],
	[
		"an instruction with two type keys",
		m({ if: 1, for: 1, do: [] } as never),
		"/do/0",
	],
	["an instruction that is only a do", m({ do: [] } as never), "/do/0"],
	["an input in an instruction position", m(ref("x") as never), "/do/0"],
	["the 0.2 format", { $: "ref", ref: "x" }, "/$"],
	["a typo'd field", { val: 1, tga: "x" }, "/tga"],
	["a missing do on if", m({ if: 1 } as never), "/do/0/do"],
	["an el on a loop", m({ for: 1, do: [], el: [] } as never), "/do/0/el"],
	[
		"an el that isn't a list",
		m({ if: 1, do: [], el: {} as never }),
		"/do/0/el",
	],
	["an instruction that isn't an object", m("set" as never), "/do/0"],
	["a literal where an instruction belongs", m(1 as never), "/do/0"],
	["a non-string tag", { val: 1, tag: 1 }, "/tag"],
	["a list as a bare input", m(set("x", [1] as never)), "/do/0/set/x"],
	["set with no variable", m({ set: {} }), "/do/0/set"],
	["set with two variables", m({ set: { a: 1, b: 2 } }), "/do/0/set"],
	["set that isn't an object", m({ set: "x" } as never), "/do/0/set"],
	["an empty variable name", m(set("", 1)), "/do/0/set"],
	["an empty reference", ref(""), "/ref"],
	["op with no call", { op: {} }, "/op"],
	["op with two calls", { op: { "math.a": {}, "math.b": {} } }, "/op"],
	["op that isn't an object", { op: "math.add" }, "/op"],
	["a call without an operation", { op: { "math.": {} } }, "/op/math."],
	["a call without an executor", { op: { ".add": {} } }, "/op/.add"],
	["a call without a dot at all", { op: { add: {} } }, "/op/add"],
	["arguments that are a list", { op: { "math.add": [] } }, "/op/math.add"],
	["a bad argument", op("math", "add", { a: [1] as never }), "/op/math.add/a"],
	[
		"the reserved key __proto__",
		JSON.parse('{"val":{"__proto__":1}}'),
		"/val/__proto__",
	],
	["the reserved key prototype", val({ prototype: 1 }), "/val/prototype"],
	[
		"a reserved variable name",
		JSON.parse('{"do":[{"set":{"__proto__":1}}]}'),
		"/do/0/set/__proto__",
	],
	[
		"a reserved argument name",
		{ op: { "math.add": { constructor: 1 } } },
		"/op/math.add/constructor",
	],
];
