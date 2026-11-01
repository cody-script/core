import { defineConfig } from "vitest/config";

export default defineConfig({
	test: {
		coverage: {
			include: ["src/**/*.ts"],
			exclude: ["src/test/**", "src/types.ts", "src/**/*.test.ts"],
			thresholds: { lines: 100, functions: 100, branches: 95, statements: 100 },
		},
	},
});
