import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

/** `wrangler dev` listens here by default; the Worker serves `/api/*` and `/ws/*`. */
const WORKER_DEV_ORIGIN = 'http://localhost:8787';

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
	}
	// WASM: `meeting-core` is built later with `wasm-pack build --target web` into
	// `src/lib/wasm/` (gitignored). The `web` target loads its `.wasm` through
	// `new URL(..., import.meta.url)`, which Vite handles natively, so no WASM plugin and
	// no top-level await are needed. `src/lib/core.ts` loads the package lazily and
	// reports its absence instead of failing the build.
});
