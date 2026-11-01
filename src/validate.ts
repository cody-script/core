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

/** Fields each node type may carry besides `$` and `tag`. A Map, so `$` can't name an `Object.prototype` member. */
const INPUTS = new Map([
	["val", ["val"]],
	["ref", ["ref"]],
	["op", ["op", "map", "arg"]],
	["m()", ["do"]],
]);
const INSTRUCTIONS = new Map([
	["set", ["set", "to"]],
	["ret", ["ret"]],
	["if", ["if", "do", "el"]],
	["for", ["if", "do"]],
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

	const operation = (n: Node, keys: Keys): void => {
		const at = [...keys, "op"];
		const dot = typeof n.op === "string" ? n.op.indexOf(".") : -1;
		if (dot < 1 || dot === (n.op as string).length - 1) {
			fail(at, 'Expected "<executor>.<operation>"');
		} else {
			const exe = (n.op as string).slice(0, dot);
			if (!hasExecutor(exe)) {
				const tag = typeof n.tag === "string" ? { tag: n.tag } : {};
				fail(at, `Unknown executor "${exe}"`, {
					code: "unknown_executor",
					...tag,
				});
			}
		}
		if (n.map !== undefined) {
			const map = record(n.map, [...keys, "map"], Object.keys(n.map as object));
			for (const [arg, from] of Object.entries(map ?? {}))
				input(from, [...keys, "map", arg]);
		}
		if (n.arg !== undefined) value(n.arg, [...keys, "arg"]);
	};

	function node(v: unknown, keys: Keys, kinds: Map<string, string[]>): void {
		if (!isObject(v)) {
			fail(keys, "Expected a node object");
			return;
		}
		const fields = typeof v.$ === "string" ? kinds.get(v.$) : undefined;
		if (!fields) {
			fail(
				[...keys, "$"],
				`Unknown node type ${JSON.stringify(v.$) ?? "undefined"}`,
			);
			return;
		}
		const n = record(v, keys, ["$", "tag", ...fields]);
		if (!n) return;
		if (n.tag !== undefined) string(n.tag, [...keys, "tag"]);
		const at = (field: string) => [...keys, field];
		switch (n.$) {
			case "val":
				value(n.val, at("val"));
				break;
			case "ref":
				name(n.ref, at("ref"));
				break;
			case "op":
				operation(n, keys);
				break;
			case "m()":
				block(n.do, at("do"));
				break;
			case "set":
				name(n.set, at("set"));
				input(n.to, at("to"));
				break;
			case "ret":
				input(n.ret, at("ret"));
				break;
			case "if":
				input(n.if, at("if"));
				block(n.do, at("do"));
				if (n.el !== undefined) block(n.el, at("el"));
				break;
			case "for":
				input(n.if, at("if"));
				block(n.do, at("do"));
				break;
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
