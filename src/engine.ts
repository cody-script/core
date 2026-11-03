import { CodyScriptError } from "./errors.js";
import { type Ast, evaluate } from "./expression.js";
import type {
	Block,
	Executor,
	Expression,
	Input,
	Instruction,
	Limits,
	Literal,
	Operation,
	Program,
	Signal,
	Value,
} from "./types.js";
import { escapeKey, validate } from "./validate.js";

// Global in every supported runtime (Workers, Node 17+, Bun) but not in the ES lib.
declare function structuredClone<T>(value: T): T;

const DEFAULT_MAX_STEPS = 100_000;
const DEFAULT_MAX_DEPTH = 128;

export type Variables = { [name: string]: Value };

export type RunOptions = {
	/** Initial variables, visible to the whole program. */
	variables?: Variables;
	/** Cancels the run between steps; passed on to executors. */
	signal?: Signal;
};

export type RunResult = {
	/** What the program returned (`null` if it never `ret`urned). */
	value: Value;
	/** The top-level variables when the program finished. */
	variables: Variables;
	/** Steps consumed, for tuning `maxSteps`. */
	steps: number;
};

/** A validated program, ready to run any number of times. */
export type CompiledProgram = {
	readonly program: Program;
	run(options?: RunOptions): Promise<RunResult>;
};

export type Engine = {
	/** Validates `program` (shape, depth, executor ids) once. Throws `CodyScriptError`. */
	compile(program: unknown): CompiledProgram;
	/** `compile(program).run(options)`. */
	run(program: unknown, options?: RunOptions): Promise<RunResult>;
};

export type EngineOptions = {
	/** The executors programs may call, by the id before the dot in `op`. Nothing else is reachable. */
	executors?: { [id: string]: Executor } | Map<string, Executor>;
	limits?: Limits;
};

/**
 * Creates an engine. Each engine owns its executors; there is no global
 * registry, so two engines (two tenants, two tests) never see each other's.
 */
export function createEngine(options: EngineOptions = {}): Engine {
	const executors = new Map<string, Executor>(
		options.executors instanceof Map
			? options.executors
			: Object.entries(options.executors ?? {}),
	);
	const maxSteps = options.limits?.maxSteps ?? DEFAULT_MAX_STEPS;
	const maxDepth = options.limits?.maxDepth ?? DEFAULT_MAX_DEPTH;

	const compile = (raw: unknown): CompiledProgram => {
		const expressions = new Map<string, Ast>();
		// Validate, then copy: later changes to `raw` can't reach a validated program.
		const program = structuredClone(
			validate(raw, maxDepth, (id) => executors.has(id), expressions),
		);
		return {
			program,
			run: (runOptions = {}) =>
				new Run(executors, expressions, maxSteps, runOptions.signal).program(
					program,
					runOptions.variables ?? {},
				),
		};
	};

	return {
		compile,
		run: (program, runOptions) => compile(program).run(runOptions),
	};
}

/** Variables, with reads falling through to the enclosing scope. */
class Scope {
	private readonly vars = new Map<string, Value>();
	constructor(private readonly parent?: Scope) {}

	get(name: string): Value | undefined {
		return this.vars.has(name) ? this.vars.get(name) : this.parent?.get(name);
	}

	set(name: string, value: Value): void {
		this.vars.set(name, value);
	}

	toObject(): Variables {
		return Object.fromEntries(this.vars);
	}
}

type Frame = { scope: Scope; done: boolean; result: Value };

/** A node is an object; anything else is a literal. */
const isNode = (input: Input): input is Exclude<Input, Literal> =>
	typeof input === "object" && input !== null;

/** The one entry of a one-key object (`set`, `op`); validate() guarantees it. */
const only = <T>(o: { [key: string]: T }): [string, T] => Object.entries(o)[0]!;

/** JS truthiness: `false`, `0`, `""` and `null` are false; everything else, including `[]` and `{}`, is true. */
const truthy = (value: Value): boolean => Boolean(value);

class Run {
	private steps = 0;

	constructor(
		private readonly executors: Map<string, Executor>,
		private readonly expressions: Map<string, Ast>,
		private readonly maxSteps: number,
		private readonly signal: Signal | undefined,
	) {}

	async program(program: Program, variables: Variables): Promise<RunResult> {
		const scope = new Scope();
		for (const [name, value] of Object.entries(variables))
			scope.set(name, value);
		// A top-level method runs in the root scope, so its assignments are the result's variables.
		const value =
			isNode(program) && "do" in program
				? await this.block(program, scope, "")
				: await this.resolve(program, scope, "");
		return { value, variables: scope.toObject(), steps: this.steps };
	}

	private step(path: string, tag: string | undefined): void {
		if (this.signal?.aborted) {
			throw new CodyScriptError("aborted", "Run aborted", {
				path,
				tag,
				cause: this.signal.reason,
			});
		}
		if (++this.steps > this.maxSteps) {
			throw new CodyScriptError(
				"step_limit",
				`Program exceeded ${this.maxSteps} steps`,
				{ path, tag },
			);
		}
	}

	private async resolve(
		input: Input,
		scope: Scope,
		path: string,
	): Promise<Value> {
		if (!isNode(input)) {
			this.step(path, undefined);
			return input;
		}
		this.step(path, input.tag);
		if ("val" in input) return input.val;
		if ("ref" in input) {
			const value = scope.get(input.ref);
			if (value === undefined) {
				throw new CodyScriptError(
					"undefined_variable",
					`Variable "${input.ref}" is not defined`,
					{ path, tag: input.tag },
				);
			}
			return value;
		}
		if ("x" in input) return this.expression(input, scope, path);
		if ("op" in input) return this.operation(input, scope, path);
		return this.block(input, new Scope(scope), path);
	}

	private expression(input: Expression, scope: Scope, path: string): Value {
		const at = `${path}/x`;
		const { tag } = input;
		const fail = (
			code: "undefined_variable" | "type_error",
			message: string,
		): never => {
			throw new CodyScriptError(code, `${message} in "${input.x}"`, {
				path: at,
				tag,
			});
		};
		// Parsed by validate() at compile time.
		return evaluate(this.expressions.get(input.x)!, {
			// `null` is a value; only a missing variable is undefined.
			get: (name) => {
				const value = scope.get(name);
				return value === undefined
					? fail("undefined_variable", `Variable "${name}" is not defined`)
					: value;
			},
			step: () => this.step(at, tag),
			fail: (message) => fail("type_error", message),
		});
	}

	private async operation(
		input: Operation,
		scope: Scope,
		path: string,
	): Promise<Value> {
		const [call, map] = only(input.op);
		const dot = call.indexOf(".");
		const exe = call.slice(0, dot);
		const op = call.slice(dot + 1);
		// Checked by validate(); the Map is the engine's own, so this cannot miss.
		const executor = this.executors.get(exe)!;
		const args: Variables = {};
		// In order, one at a time: arguments may have side effects.
		for (const [arg, from] of Object.entries(map)) {
			const at = `${path}/op/${escapeKey(call)}/${escapeKey(arg)}`;
			args[arg] = await this.resolve(from, scope, at);
		}
		try {
			const result = await executor({
				op,
				...(input.arg !== undefined && { arg: input.arg }),
				input: args,
				...(this.signal && { signal: this.signal }),
			});
			return result === undefined ? null : result;
		} catch (cause) {
			if (cause instanceof CodyScriptError) throw cause;
			const reason = cause instanceof Error ? cause.message : String(cause);
			throw new CodyScriptError(
				"executor_failed",
				`Executor "${exe}" failed on "${op}": ${reason}`,
				{ path, tag: input.tag, cause },
			);
		}
	}

	private async block(
		block: Block,
		scope: Scope,
		path: string,
	): Promise<Value> {
		const frame: Frame = { scope, done: false, result: null };
		await this.list(block.do, frame, `${path}/do`);
		return frame.result;
	}

	private async list(
		list: Instruction[],
		frame: Frame,
		path: string,
	): Promise<void> {
		for (let i = 0; i < list.length && !frame.done; i++) {
			await this.execute(list[i]!, frame, `${path}/${i}`);
		}
	}

	private async execute(
		ins: Instruction,
		frame: Frame,
		path: string,
	): Promise<void> {
		this.step(path, ins.tag);
		const { scope } = frame;
		if ("set" in ins) {
			const [name, to] = only(ins.set);
			scope.set(
				name,
				await this.resolve(to, scope, `${path}/set/${escapeKey(name)}`),
			);
		} else if ("ret" in ins) {
			frame.result = await this.resolve(ins.ret, scope, `${path}/ret`);
			frame.done = true;
		} else if ("if" in ins) {
			if (truthy(await this.resolve(ins.if, scope, `${path}/if`))) {
				await this.list(ins.do, frame, `${path}/do`);
			} else if (ins.el) {
				await this.list(ins.el, frame, `${path}/el`);
			}
		} else {
			while (
				!frame.done &&
				truthy(await this.resolve(ins.for, scope, `${path}/for`))
			) {
				await this.list(ins.do, frame, `${path}/do`);
			}
		}
	}
}
