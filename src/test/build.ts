import type {
	Constant,
	Executor,
	Input,
	Instruction,
	Method,
	Operation,
	Reference,
	Value,
} from "../types.js";

/** Terse builders so tests read like programs. */
export const val = (v: Value): Constant => ({ $: "val", val: v });
export const ref = (name: string): Reference => ({ $: "ref", ref: name });
export const op = (
	exe: string,
	id: string,
	map?: { [k: string]: Input },
	arg?: Value,
): Operation => ({
	$: "op",
	op: `${exe}.${id}`,
	...(map && { map }),
	...(arg !== undefined && { arg }),
});
export const m = (...body: Instruction[]): Method => ({ $: "m()", do: body });
export const set = (name: string, to: Input): Instruction => ({
	$: "set",
	set: name,
	to,
});
export const ret = (input: Input): Instruction => ({ $: "ret", ret: input });
export const when = (
	cond: Input,
	then: Instruction[],
	otherwise?: Instruction[],
): Instruction => ({
	$: "if",
	if: cond,
	do: then,
	...(otherwise && { el: otherwise }),
});
export const loop = (cond: Input, body: Instruction[]): Instruction => ({
	$: "for",
	if: cond,
	do: body,
});

/** A tiny arithmetic/comparison executor: `op("math", "add", { a, b })`. */
export const math: Executor = ({ op, input = {} }) => {
	const a = input.a as number;
	const b = input.b as number;
	switch (op) {
		case "add":
			return a + b;
		case "lt":
			return a < b;
		case "eq":
			return a === b;
		default:
			throw new Error(`unknown op ${op}`);
	}
};
