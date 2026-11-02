import { CodyScriptError } from "./errors.js";
import type { Program } from "./types.js";

type Keys = (string | number)[];
type Issue = {
	path: string;
	message: string;
	code: "invalid_program" | "unknown_executor";
	tag?: string;
};
type Node = { [key: string]: unknown };

/**
 * Node types by the key that names them, with the other fields each may carry
 * besides `tag`. A node has exactly one type key; its position (input or
 * instruction) decides which set applies.
 */
const INPUTS = new Map([
	["val", []],
	["ref", []],
	["op", ["arg"]],
	["do", []],
]);
const INSTRUCTIONS = new Map([
	["set", []],
	["ret", []],
	["if", ["do", "el"]],
	["for", ["do"]],
]);

/** Keys no object may use; refused rather than risk `Object.prototype` reaching a program. */
const RESERVED = new Set(["__proto__", "prototype", "constructor"]);

/** Escapes one JSON Pointer (RFC 6901) reference token. */
export const escapeKey = (key: string | number): string =>
	String(key).replaceAll("~", "~0").replaceAll("/", "~1");

/** JSON Pointer for a list of keys. */
export const pointer = (keys: readonly (string | number)[]): string =>
	keys.map((k) => `/${escapeKey(k)}`).join("");

/**
 * Checks `program` in one walk — shape, unknown fields, JSON-only values,
 * nesting depth, reserved keys and executor ids — and reports every problem.
 * Throws `invalid_program` or `unknown_executor` (whichever comes first).
 */
export function validate(
	program: unknown,
	maxDepth: number,
	hasExecutor: (id: string) => boolean,
): Program {
	const issues: Issue[] = [];
	const fail = (keys: Keys, message: string, extra?: Partial<Issue>) =>
		issues.push({
			path: pointer(keys),
			message,
			code: "invalid_program",
			...extra,
		});

	/** An object or array we may descend into: not too deep, no reserved keys. */
	const container = (v: unknown, keys: Keys): boolean => {
		if (keys.length >= maxDepth) {
			fail(keys, `Program nests deeper than ${maxDepth} levels`);
			return false;
		}
		const reserved = Object.keys(v as object).find((k) => RESERVED.has(k));
		if (reserved !== undefined) {
			fail([...keys, reserved], `"${reserved}" is a reserved key`);
			return false;
		}
		return true;
	};

	const isObject = (v: unknown): v is Node =>
		typeof v === "object" && v !== null && !Array.isArray(v);

	/** A plain object with only `allowed` fields (plus `$`/`tag` on nodes). */
	const record = (
		v: unknown,
		keys: Keys,
		allowed: string[],
	): Node | undefined => {
		if (!isObject(v)) {
			fail(keys, "Expected an object");
			return undefined;
		}
		if (!container(v, keys)) return undefined;
		for (const k of Object.keys(v)) {
			if (!allowed.includes(k)) fail([...keys, k], `Unknown field "${k}"`);
		}
		return v;
	};

	const name = (v: unknown, keys: Keys): v is string => {
		if (typeof v === "string" && v.length > 0) return true;
		fail(keys, "Expected a non-empty string");
		return false;
	};

	const string = (v: unknown, keys: Keys): void => {
		if (typeof v !== "string") fail(keys, "Expected a string");
	};

	const value = (v: unknown, keys: Keys): void => {
		if (v === null || typeof v === "boolean" || typeof v === "string") return;
		if (typeof v === "number") {
			if (!Number.isFinite(v)) fail(keys, "Expected a finite number");
			return;
		}
		const proto = typeof v === "object" ? Object.getPrototypeOf(v) : undefined;
		if (Array.isArray(v) || proto === Object.prototype || proto === null) {
			if (!container(v, keys)) return;
			for (const [k, child] of Object.entries(v as object))
				value(child, [...keys, k]);
			return;
		}
		fail(keys, "Expected a JSON value");
	};

	const block = (v: unknown, keys: Keys): void => {
		if (!Array.isArray(v)) {
			fail(keys, "Expected a list of instructions");
			return;
		}
		if (!container(v, keys)) return;
		v.forEach((ins, i) => node(ins, [...keys, i], INSTRUCTIONS));
	};

	/** A literal (`null`, boolean, finite number, string) or an input node. */
	const input = (v: unknown, keys: Keys): void => {
		if (typeof v === "object" && v !== null) node(v, keys, INPUTS);
		else value(v, keys);
	};

	/** A one-key object (`set`, `op`): its single entry, or undefined. */
	const single = (v: unknown, keys: Keys): [string, unknown] | undefined => {
		if (!isObject(v)) {
			fail(keys, "Expected an object");
			return undefined;
		}
		if (!container(v, keys)) return undefined;
		const entries = Object.entries(v);
		if (entries.length === 1) return entries[0];
		fail(keys, `Expected exactly one key, found ${entries.length}`);
		return undefined;
	};

	const operation = (n: Node, keys: Keys): void => {
		const entry = single(n.op, [...keys, "op"]);
		if (entry) {
			const [call, args] = entry;
			const at = [...keys, "op", call];
			const dot = call.indexOf(".");
			if (dot < 1 || dot === call.length - 1) {
				fail(at, 'Expected "<executor>.<operation>"');
			} else if (!hasExecutor(call.slice(0, dot))) {
				const tag = typeof n.tag === "string" ? { tag: n.tag } : {};
				fail(at, `Unknown executor "${call.slice(0, dot)}"`, {
					code: "unknown_executor",
					...tag,
				});
			}
			const map = record(args, at, Object.keys(isObject(args) ? args : {}));
			for (const [arg, from] of Object.entries(map ?? {}))
				input(from, [...at, arg]);
		}
		if (n.arg !== undefined) value(n.arg, [...keys, "arg"]);
	};

	function node(v: unknown, keys: Keys, kinds: Map<string, string[]>): void {
		const what = kinds === INPUTS ? "an input" : "an instruction";
		if (!isObject(v)) {
			fail(keys, `Expected ${what} object`);
			return;
		}
		const types = [...kinds.keys()].filter((k) => Object.hasOwn(v, k));
		if (types.length !== 1) {
			const expected = [...kinds.keys()].map((k) => `"${k}"`).join(", ");
			const found = types.length
				? `, found ${types.map((k) => `"${k}"`).join(" and ")}`
				: "";
			fail(keys, `Expected ${what} with exactly one of ${expected}${found}`);
			return;
		}
		const type = types[0]!;
		const n = record(v, keys, [type, "tag", ...kinds.get(type)!]);
		if (!n) return;
		if (n.tag !== undefined) string(n.tag, [...keys, "tag"]);
		const at = (field: string) => [...keys, field];
		switch (type) {
			case "val":
				value(n.val, at("val"));
				break;
			case "ref":
				name(n.ref, at("ref"));
				break;
			case "op":
				operation(n, keys);
				break;
			case "set": {
				const entry = single(n.set, at("set"));
				if (entry && name(entry[0], at("set")))
					input(entry[1], [...keys, "set", entry[0]]);
				break;
			}
			case "ret":
				input(n.ret, at("ret"));
				break;
			case "if":
			case "for":
				input(n[type], at(type));
				block(n.do, at("do"));
				if (n.el !== undefined) block(n.el, at("el"));
				break;
			default: // "do": a block
				block(n.do, at("do"));
		}
	}

	input(program, []);
	const [first] = issues;
	if (!first) return program as Program;
	const prefix = first.code === "invalid_program" ? "Invalid program: " : "";
	throw new CodyScriptError(first.code, `${prefix}${first.message}`, {
		path: first.path,
		...(first.tag !== undefined && { tag: first.tag }),
		issues: issues.map(({ path, message }) => ({ path, message })),
	});
}
