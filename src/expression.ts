import type { Value } from "./types.js";

/**
 * The expression language `{ "x": "…" }` nodes use: numbers, 'text', true,
 * false, null, variable names, + - * /, parentheses, one comparison
 * (== != < <= > >=), && || and !. Parsed once at compile time by hand —
 * never evaluated as JavaScript — and evaluated against the run's scope.
 */
export type Ast =
	| { v: Value } // a literal
	| { n: string } // a variable
	| { u: "!" | "-"; a: Ast } // a unary operator
	| { b: string; a: Ast; c: Ast }; // a binary operator

/** Binary operators by precedence, loosest first. */
const LEVELS = [
	["||"],
	["&&"],
	["==", "!=", "<", "<=", ">", ">="],
	["+", "-"],
	["*", "/"],
];
const COMPARISON = 2;
const KEYWORDS = new Map<string, Value>([
	["true", true],
	["false", false],
	["null", null],
]);
const TOKEN =
	/\s*(?:(\d+(?:\.\d+)?)|'((?:[^'\\]|\\.)*)'|([A-Za-z_]\w*)|(==|!=|<=|>=|&&|\|\||[-+*/()<>!]))/y;

type Token = { v: Value } | { s: string };

/** Parses `source`; throws an Error whose message says what's wrong. */
export function parse(source: string, maxDepth: number): Ast {
	const tokens: Token[] = [];
	TOKEN.lastIndex = 0;
	while (TOKEN.lastIndex < source.length) {
		const at = TOKEN.lastIndex;
		const m = TOKEN.exec(source);
		if (!m) {
			if (!source.slice(at).trim()) break;
			throw new Error(`Unexpected "${source.slice(at).trim()[0]}"`);
		}
		const [, num, text, word, op] = m;
		if (num !== undefined) tokens.push({ v: Number(num) });
		else if (text !== undefined)
			tokens.push({ v: text.replace(/\\(.)/g, "$1") });
		else if (word !== undefined && KEYWORDS.has(word))
			tokens.push({ v: KEYWORDS.get(word)! });
		else tokens.push({ s: (word ?? op)! });
	}

	let i = 0;
	let depth = 0;
	const peek = (): string | undefined => {
		const t = tokens[i];
		return t && "s" in t ? t.s : undefined;
	};
	const deeper = () => {
		if (++depth > maxDepth)
			throw new Error(`Expression nests deeper than ${maxDepth} levels`);
	};

	const level = (p: number): Ast => {
		if (p === LEVELS.length) return unary();
		let left = level(p + 1);
		while (LEVELS[p]!.includes(peek()!)) {
			const op = peek()!;
			i++;
			left = { b: op, a: left, c: level(p + 1) };
			if (p === COMPARISON) {
				if (LEVELS[p]!.includes(peek()!))
					throw new Error("Comparisons can't be chained; use &&");
				break;
			}
		}
		return left;
	};

	const unary = (): Ast => {
		deeper();
		const t = tokens[i++];
		let ast: Ast;
		if (t === undefined) throw new Error("Expression ends too early");
		if ("v" in t) ast = t;
		else if (t.s === "!" || t.s === "-") ast = { u: t.s, a: unary() };
		else if (t.s === "(") {
			ast = level(0);
			if (peek() !== ")") throw new Error('Missing ")"');
			i++;
		} else if (/^[A-Za-z_]/.test(t.s)) ast = { n: t.s };
		else throw new Error(`Unexpected "${t.s}"`);
		depth--;
		return ast;
	};

	const ast = level(0);
	if (i < tokens.length) {
		const t = tokens[i]!;
		throw new Error(
			`Unexpected ${"s" in t ? `"${t.s}"` : JSON.stringify(t.v)}`,
		);
	}
	return ast;
}

/** What `evaluate` needs from the engine. */
export type Context = {
	/** A variable's value; throws if it isn't defined. */
	get(name: string): Value;
	/** Counts one step; throws past the limit or once aborted. */
	step(): void;
	/** Throws a type error with this message. */
	fail(message: string): never;
};

/** An array or object: not comparable with == or ordering. */
const isList = (v: Value) => typeof v === "object" && v !== null;
const kind = (v: Value) =>
	v === null ? "null" : Array.isArray(v) ? "list" : typeof v;

/** Evaluates `ast`. `&&` and `||` short-circuit and return an operand, as in JavaScript. */
export function evaluate(ast: Ast, cx: Context): Value {
	cx.step();
	if ("v" in ast) return ast.v;
	if ("n" in ast) return cx.get(ast.n);
	if ("u" in ast) {
		const a = evaluate(ast.a, cx);
		if (ast.u === "!") return !a;
		if (typeof a !== "number") cx.fail(`"-" needs a number, got ${kind(a)}`);
		return -a;
	}
	const op = ast.b;
	const a = evaluate(ast.a, cx);
	if (op === "&&" || op === "||")
		return (op === "&&") === Boolean(a) ? evaluate(ast.c, cx) : a;
	const c = evaluate(ast.c, cx);
	const pair = `${kind(a)} and ${kind(c)}`;
	if (op === "==" || op === "!=") {
		if (isList(a) || isList(c)) cx.fail(`"${op}" can't compare ${pair}`);
		return (a === c) === (op === "==");
	}
	const same = typeof a === typeof c;
	const order = ORDER.get(op);
	if (order) {
		if (!same || (typeof a !== "number" && typeof a !== "string")) {
			cx.fail(`"${op}" needs two numbers or two texts, got ${pair}`);
		}
		return order(a as number, c as number);
	}
	if (!same || typeof a !== "number")
		cx.fail(`"${op}" needs numbers, got ${pair}`);
	const result = MATH.get(op)!(a as number, c as number);
	if (!Number.isFinite(result))
		cx.fail(`"${op}" gave ${result} (division by zero?)`);
	return result;
}

type Fn<T> = (x: number, y: number) => T;
const ORDER = new Map<string, Fn<boolean>>([
	["<", (x, y) => x < y],
	["<=", (x, y) => x <= y],
	[">", (x, y) => x > y],
	[">=", (x, y) => x >= y],
]);
const MATH = new Map<string, Fn<number>>([
	["+", (x, y) => x + y],
	["-", (x, y) => x - y],
	["*", (x, y) => x * y],
	["/", (x, y) => x / y],
]);
