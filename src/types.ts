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

// ── Inputs: what produces a value ───────────────────────────────────────────

/** A bare number, string, boolean or `null` is its own value. `42`, `"text"` */
export type Literal = null | boolean | number | string;

/** Any value, including objects and arrays. `{ "$": "val", "val": { "a": 1 } }` */
export type Constant = Tagged & { $: "val"; val: Value };

/** Reads a variable. `{ "$": "ref", "ref": "total" }` */
export type Reference = Tagged & { $: "ref"; ref: string };

/**
 * Calls a host-provided executor. `op` is `"<executor>.<operation>"`, split at
 * the first dot; `map` supplies computed arguments and `arg` a static one.
 * `{ "$": "op", "op": "math.add", "map": { "a": 1, "b": 2 } }`
 */
export type Operation = Tagged & {
	$: "op";
	op: string;
	map?: { [arg: string]: Input };
	arg?: Value;
};

/** A block with its own scope; its value is whatever it `ret`urns (else `null`). */
export type Method = Tagged & { $: "m()"; do: Instruction[] };

export type Input = Literal | Constant | Reference | Operation | Method;

// ── Instructions: nodes that do something ───────────────────────────────────

/** Assigns a variable in the current scope. */
export type Set = Tagged & { $: "set"; set: string; to: Input };

/** Ends the enclosing method with a value. */
export type Return = Tagged & { $: "ret"; ret: Input };

/** Runs `do` when `if` is truthy, otherwise `el`. */
export type Condition = Tagged & {
	$: "if";
	if: Input;
	do: Instruction[];
	el?: Instruction[];
};

/** Runs `do` while `if` is truthy. */
export type Loop = Tagged & { $: "for"; if: Input; do: Instruction[] };

export type Instruction = Set | Return | Condition | Loop;

/** A program is any input; usually a method. */
export type Program = Input;

// ── Host side ───────────────────────────────────────────────────────────────

/** The subset of `AbortSignal` the engine reads. */
export type Signal = { readonly aborted: boolean; readonly reason?: unknown };

/** What an executor is called with. */
export type ExecutorCall = {
	/** The operation: what follows the executor id in `op`. */
	op: string;
	/** `arg` from the program, if any. */
	arg?: Value;
	/** `map`, resolved, if any. */
	input?: { [arg: string]: Value };
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
