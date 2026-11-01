import * as v from "valibot";
import { CodyScriptError } from "./errors.js";
import type { Input, Instruction, Program, Value } from "./types.js";

const tag = v.optional(v.string());
const name = v.pipe(v.string(), v.minLength(1));

export const ValueSchema: v.GenericSchema<Value> = v.lazy(() =>
	v.union([
		v.null(),
		v.boolean(),
		v.pipe(v.number(), v.finite()),
		v.string(),
		v.array(ValueSchema),
		v.record(v.string(), ValueSchema),
	]),
);

const block: v.GenericSchema<Instruction[]> = v.lazy(() =>
	v.array(InstructionSchema),
);

export const InputSchema: v.GenericSchema<Input> = v.lazy(() =>
	v.variant("$", [
		v.strictObject({ $: v.literal("val"), val: ValueSchema, tag }),
		v.strictObject({ $: v.literal("ref"), ref: name, tag }),
		v.strictObject({
			$: v.literal("op"),
			op: v.strictObject({
				id: v.string(),
				map: v.optional(v.record(v.string(), InputSchema)),
			}),
			exe: v.strictObject({ id: name, arg: v.optional(ValueSchema) }),
			tag,
		}),
		v.strictObject({
			$: v.literal("m()"),
			id: v.optional(v.string()),
			do: block,
			tag,
		}),
	]),
);

export const InstructionSchema: v.GenericSchema<Instruction> = v.lazy(() =>
	v.variant("$", [
		v.strictObject({ $: v.literal("set"), set: name, to: InputSchema, tag }),
		v.strictObject({ $: v.literal("ret"), ret: InputSchema, tag }),
		v.strictObject({
			$: v.literal("if"),
			if: InputSchema,
			do: block,
			el: v.optional(block),
			tag,
		}),
		v.strictObject({ $: v.literal("for"), if: InputSchema, do: block, tag }),
	]),
);

/** Escapes one JSON Pointer (RFC 6901) reference token. */
export const escapeKey = (key: string | number): string =>
	String(key).replaceAll("~", "~0").replaceAll("/", "~1");

/** JSON Pointer for a list of keys. */
export const pointer = (keys: readonly (string | number)[]): string =>
	keys.map((k) => `/${escapeKey(k)}`).join("");

/** Keys a JSON object may not use: the validator would silently drop them. */
const RESERVED_KEYS = new Set(["__proto__", "prototype", "constructor"]);

/**
 * A bounded walk before validation, so a hostile document can't overflow the
 * validator's stack, and a reserved key is refused rather than quietly lost.
 */
function preflight(
	node: unknown,
	max: number,
	keys: (string | number)[] = [],
): { path: string; message: string } | undefined {
	if (node === null || typeof node !== "object") return undefined;
	if (keys.length >= max) {
		return {
			path: pointer(keys),
			message: `Program nests deeper than ${max} levels`,
		};
	}
	for (const [k, child] of Object.entries(node)) {
		if (RESERVED_KEYS.has(k)) {
			return {
				path: pointer([...keys, k]),
				message: `"${k}" is a reserved key`,
			};
		}
		const found = preflight(child, max, [...keys, k]);
		if (found) return found;
	}
	return undefined;
}

/** Checks `program` against the language's shape; throws `invalid_program`. */
export function parseProgram(program: unknown, maxDepth: number): Program {
	const refused = preflight(program, maxDepth);
	if (refused) {
		throw new CodyScriptError("invalid_program", refused.message, {
			path: refused.path,
			issues: [refused],
		});
	}
	const result = v.safeParse(InputSchema, program);
	if (result.success) return result.output;
	const issues = result.issues.map((issue) => ({
		path: pointer(
			(issue.path ?? []).map((item) => item.key as string | number),
		),
		message: issue.message,
	}));
	const first = issues[0]!;
	throw new CodyScriptError(
		"invalid_program",
		`Invalid program: ${first.message}`,
		{
			path: first.path,
			issues,
		},
	);
}
