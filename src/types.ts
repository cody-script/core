/** Any JSON value. Everything a program reads, computes or returns is one. */
export type Value =
	| null
	| boolean
	| number
	| string
	| Value[]
	| { [key: string]: Value };

/** Every node may carry a `tag`: an opaque label echoed back in errors (an editor's node id, a rule name). */
type Tagged = { tag?: string };

// Every node is an object whose type is its key: `{ "ref": "x" }` is a
// reference, `{ "set": { "x": 1 } }` an assignment. Objects and arrays in an
// input position are always nodes; a constant object is wrapped in `val`.

// ── Inputs: what produces a value ───────────────────────────────────────────

/** A bare number, string, boolean or `null` is its own value. `42`, `"text"` */
export type Literal = null | boolean | number | string;

/** Any value, including objects and arrays. `{ "val": { "a": 1 } }` */
export type Constant = Tagged & { val: Value };

/** Reads a variable. `{ "ref": "total" }` */
export type Reference = Tagged & { ref: string };

/**
 * Calls a host-provided executor. `op` has one key, `"<executor>.<operation>"`
 * (split at the first dot), naming the arguments to compute; `arg` is a static
 * one. `{ "op": { "math.add": { "a": 1, "b": 2 } } }`
 */
export type Operation = Tagged & {
	op: { [call: string]: { [arg: string]: Input } };
	arg?: Value;
};

/** A block with its own scope; its value is whatever it `ret`urns (else `null`). `{ "do": [...] }` */
export type Block = Tagged & { do: Instruction[] };

/**
 * An expression over variables: arithmetic, one comparison, `&&` `||` `!`.
 * Parsed at compile time. `{ "x": "gross - discounts > 0" }`
 */
export type Expression = Tagged & { x: string };

export type Input =
	| Literal
	| Constant
	| Reference
	| Expression
	| Operation
	| Block;

// ── Instructions: nodes that do something ───────────────────────────────────

/** Assigns one variable in the current scope. `{ "set": { "total": 0 } }` */
export type Set = Tagged & { set: { [name: string]: Input } };

/** Ends the enclosing block with a value. `{ "ret": { "ref": "total" } }` */
export type Return = Tagged & { ret: Input };

/** Runs `do` when `if` is truthy, otherwise `el`. */
export type Condition = Tagged & {
	if: Input;
	do: Instruction[];
	el?: Instruction[];
};

/** Runs `do` while `for` is truthy. */
export type Loop = Tagged & { for: Input; do: Instruction[] };

export type Instruction = Set | Return | Condition | Loop;

/** A program is any input; usually a block. */
export type Program = Input;

// ── Host side ───────────────────────────────────────────────────────────────

/** The subset of `AbortSignal` the engine reads. */
export type Signal = { readonly aborted: boolean; readonly reason?: unknown };

/** What an executor is called with. */
export type ExecutorCall = {
	/** The operation: what follows the executor id in the call. */
	op: string;
	/** `arg` from the program, if any. */
	arg?: Value;
	/** The call's arguments, resolved (`{}` when it has none). */
	input: { [arg: string]: Value };
	/** The run's abort signal, if one was given. */
	signal?: Signal;
};

/**
 * Host code a program may call. Must return a JSON value; `undefined` is
 * treated as `null`. Throwing fails the run with `executor_failed`.
 */
export type Executor = (
	call: ExecutorCall,
) => Value | undefined | Promise<Value | undefined>;

/** Bounds on what one program may consume. */
export type Limits = {
	/** Nodes evaluated per run (each instruction, input and loop test). Default 100 000. */
	maxSteps?: number;
	/** Maximum JSON nesting depth of a program. Default 128. */
	maxDepth?: number;
};
