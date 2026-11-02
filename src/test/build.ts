import type {
	Block,
	Constant,
	Executor,
	Input,
	Instruction,
	Operation,
	Reference,
	Value,
} from "../types.js";

/** Terse builders so tests read like programs. */
export const val = (v: Value): Constant => ({ val: v });
export const ref = (name: string): Reference => ({ ref: name });
export const op = (
	exe: string,
	id: string,
	args: { [k: string]: Input } = {},
	arg?: Value,
): Operation => ({
	op: { [`${exe}.${id}`]: args },
	...(arg !== undefined && { arg }),
});
export const m = (...body: Instruction[]): Block => ({ do: body });
export const set = (name: string, to: Input): Instruction => ({
	set: { [name]: to },
});
export const ret = (input: Input): Instruction => ({ ret: input });
export const when = (
	cond: Input,
	then: Instruction[],
	otherwise?: Instruction[],
): Instruction => ({
	if: cond,
	do: then,
	...(otherwise && { el: otherwise }),
});
export const loop = (cond: Input, body: Instruction[]): Instruction => ({
	for: cond,
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
