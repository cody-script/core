export type ErrorCode =
	| "invalid_program"
	| "unknown_executor"
	| "undefined_variable"
	| "executor_failed"
	| "step_limit"
	| "aborted";

/**
 * Every failure the engine raises. `path` is a JSON Pointer into the program
 * (`/do/2/to`) and `tag` the node's tag, so a host can point at the culprit.
 */
export class CodyScriptError extends Error {
	override readonly name = "CodyScriptError";
	readonly code: ErrorCode;
	readonly path: string;
	readonly tag?: string;
	/** For `invalid_program`: every problem found, not just the first. */
	readonly issues?: { path: string; message: string }[];

	constructor(
		code: ErrorCode,
		message: string,
		details: {
			path?: string;
			tag?: string;
			issues?: { path: string; message: string }[];
			cause?: unknown;
		} = {},
	) {
		const where = details.path ? ` at ${details.path}` : "";
		const tag = details.tag ? ` (tag ${details.tag})` : "";
		super(`${message}${where}${tag}`, { cause: details.cause });
		this.code = code;
		this.path = details.path ?? "";
		if (details.tag !== undefined) this.tag = details.tag;
		if (details.issues) this.issues = details.issues;
	}
}
