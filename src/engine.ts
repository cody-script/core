import { CodyScriptError } from "./errors.js";
import type {
	Executor,
	Input,
	Instruction,
	Limits,
	Method,
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
	/** The operations programs may call, by `exe.id`. Nothing else is reachable. */
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
		// Validate, then copy: later changes to `raw` can't reach a validated program.
		const program = structuredClone(
			validate(raw, maxDepth, (id) => executors.has(id)),
		);
		return {
			program,
			run: (runOptions = {}) =>
				new Run(executors, maxSteps, runOptions.signal).program(
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

/** JS truthiness: `false`, `0`, `""` and `null` are false; everything else, including `[]` and `{}`, is true. */
const truthy = (value: Value): boolean => Boolean(value);

class Run {
	private steps = 0;

	constructor(
		private readonly executors: Map<string, Executor>,
		private readonly maxSteps: number,
		private readonly signal: Signal | undefined,
	) {}

	async program(program: Program, variables: Variables): Promise<RunResult> {
		const scope = new Scope();
		for (const [name, value] of Object.entries(variables))
			scope.set(name, value);
		// A top-level method runs in the root scope, so its assignments are the result's variables.
		const value =
			program.$ === "m()"
				? await this.method(program, scope, "")
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
		this.step(path, input.tag);
		switch (input.$) {
			case "val":
				return input.val;
			case "ref": {
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
			case "op":
				return this.operation(input, scope, path);
			case "m()":
				return this.method(input, new Scope(scope), path);
		}
	}

	private async operation(
		input: Extract<Input, { $: "op" }>,
		scope: Scope,
		path: string,
	): Promise<Value> {
		// Checked by validate(); the Map is the engine's own, so this cannot miss.
		const executor = this.executors.get(input.exe.id)!;
		let args: Variables | undefined;
		if (input.op.map) {
			args = {};
			// In order, one at a time: arguments may have side effects.
			for (const [arg, from] of Object.entries(input.op.map)) {
				const value = await this.resolve(
					from,
					scope,
					`${path}/op/map/${escapeKey(arg)}`,
				);
				args[arg] = value;
			}
		}
		try {
			const result = await executor({
				op: input.op.id,
				...(input.exe.arg !== undefined && { arg: input.exe.arg }),
				...(args && { input: args }),
				...(this.signal && { signal: this.signal }),
			});
			return result === undefined ? null : result;
		} catch (cause) {
			if (cause instanceof CodyScriptError) throw cause;
			const reason = cause instanceof Error ? cause.message : String(cause);
			throw new CodyScriptError(
				"executor_failed",
				`Executor "${input.exe.id}" failed on "${input.op.id}": ${reason}`,
				{ path, tag: input.tag, cause },
			);
		}
	}

	private async method(
		method: Method,
		scope: Scope,
		path: string,
	): Promise<Value> {
		const frame: Frame = { scope, done: false, result: null };
		await this.block(method.do, frame, `${path}/do`);
		return frame.result;
	}

	private async block(
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
		switch (ins.$) {
			case "set":
				frame.scope.set(
					ins.set,
					await this.resolve(ins.to, frame.scope, `${path}/to`),
				);
				return;
			case "ret":
				frame.result = await this.resolve(ins.ret, frame.scope, `${path}/ret`);
				frame.done = true;
				return;
			case "if":
				if (truthy(await this.resolve(ins.if, frame.scope, `${path}/if`))) {
					await this.block(ins.do, frame, `${path}/do`);
				} else if (ins.el) {
					await this.block(ins.el, frame, `${path}/el`);
				}
				return;
			case "for":
				while (
					!frame.done &&
					truthy(await this.resolve(ins.if, frame.scope, `${path}/if`))
				) {
					await this.block(ins.do, frame, `${path}/do`);
				}
				return;
		}
	}
}
