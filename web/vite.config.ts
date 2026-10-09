import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

/** `wrangler dev` listens here by default; the Worker serves `/api/*` and `/ws/*`. */
const WORKER_DEV_ORIGIN = 'http://localhost:8787';

/**
 * `meeting-core` as WASM, with the protocol types (see `src/lib/core.ts`). The `web` target
 * loads its `.wasm` through `new URL(..., import.meta.url)`, which Vite handles natively, so
 * no WASM plugin and no top-level await are needed.
 */
const CORE_PACKAGE = fileURLToPath(new URL('./src/lib/wasm/meeting_core.js', import.meta.url));
if (!existsSync(CORE_PACKAGE)) {
	throw new Error(
		'The meeting-core WASM package is missing (src/lib/wasm). ' +
			'Build it with `pnpm --filter web build:core` (needs Rust and wasm-pack), then retry.'
	);
}

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for our code; libraries decide for themselves.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// Fully static SPA: every route renders in the browser from `index.html`.
			// The Worker serves `build/` as static assets with SPA fallback.
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	server: {
		proxy: {
			'/api': { target: WORKER_DEV_ORIGIN, changeOrigin: true },
			'/ws': { target: WORKER_DEV_ORIGIN, changeOrigin: true, ws: true }
		}
	},
	test: {
		include: ['src/**/*.test.ts'],
		environment: 'node'
	}
});
