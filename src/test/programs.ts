import { m, op, ref, set, val, when } from "./build.js";

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
	["a reference", ref("x")],
	["a call with no arguments", op("math", "now")],
	[
		"a call with map and arg",
		op("math", "add", { a: 1, b: ref("x") }, { round: true }),
	],
	["an operation whose name has dots", op("math", "round.half.up")],
	["an empty method", m()],
	[
		"every node type, tagged",
		{
			...m(
				{ $: "set", set: "a", to: { $: "val", val: 1, tag: "v" }, tag: "s" },
				{
					...when(ref("a"), [], [{ $: "for", if: false, do: [], tag: "f" }]),
					tag: "i",
				},
				{
					$: "ret",
					ret: { ...op("math", "add", { a: 1, b: 2 }), tag: "o" },
					tag: "r",
				},
			),
			tag: "root",
		},
	],
];

/** Programs both must refuse, with the path the validator reports first. */
export const invalid: [string, unknown, string][] = [
	["an unknown node type", { $: "while" }, "/$"],
	["a missing $", { val: 1 }, "/$"],
	["a missing field", { $: "ref" }, "/ref"],
	["a typo'd field", { $: "val", val: 1, tga: "x" }, "/tga"],
	["the removed method id", { $: "m()", do: [], id: "main" }, "/id"],
	[
		"the old nested op shape",
		{ $: "op", op: { id: "add" }, exe: { id: "math" } },
		"/exe",
	],
	["an empty variable name", m(set("", 1)), "/do/0/set"],
	["a list as a bare input", m(set("x", [1] as never)), "/do/0/to"],
	[
		"an el that isn't a list",
		m({ $: "if", if: 1, do: [], el: {} as never }),
		"/do/0/el",
	],
	["an instruction that isn't an object", m("set" as never), "/do/0"],
	["a literal where an instruction belongs", m(1 as never), "/do/0"],
	["an input where an instruction belongs", m(val(1) as never), "/do/0/$"],
	["a non-string tag", { $: "val", val: 1, tag: 1 }, "/tag"],
	["op without a dot", { $: "op", op: "add" }, "/op"],
	["op without an executor", { $: "op", op: ".add" }, "/op"],
	["op without an operation", { $: "op", op: "math." }, "/op"],
	["op that isn't a string", { $: "op", op: 1 }, "/op"],
	["a map that is a list", { $: "op", op: "math.add", map: [] }, "/map"],
	["a bad map argument", op("math", "add", { a: [1] as never }), "/map/a"],
	[
		"the reserved key __proto__",
		JSON.parse('{"$":"val","val":{"__proto__":1}}'),
		"/val/__proto__",
	],
	["the reserved key prototype", val({ prototype: 1 }), "/val/prototype"],
	[
		"the reserved key constructor in a map",
		{ $: "op", op: "math.add", map: { constructor: 1 } },
		"/map/constructor",
	],
];
