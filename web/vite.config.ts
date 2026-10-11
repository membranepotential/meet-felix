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
			adapter: adapter({ fallback: 'index.html' }),
			// The pages' Content-Security-Policy, as a `<meta>` tag with the hash of SvelteKit's
			// inline bootstrap script. A meta tag cannot carry `frame-ancestors`; `static/_headers`
			// sends that, with `nosniff`, for every static file.
			csp: {
				mode: 'hash',
				directives: {
					'default-src': ['self'],
					// The meeting core is WebAssembly, which needs `wasm-unsafe-eval` to compile.
					'script-src': ['self', 'wasm-unsafe-eval'],
					// Svelte writes `style` attributes, and `app.html` has one.
					'style-src': ['self', 'unsafe-inline'],
					// The API and the room's WebSocket are same-origin; media goes over
					// `RTCPeerConnection`, which `connect-src` does not govern.
					'connect-src': ['self'],
					'img-src': ['self', 'data:'],
					'font-src': ['self'],
					'object-src': ['none'],
					'base-uri': ['self'],
					'form-action': ['self']
				}
			}
		})
	],
	server: {
		// The Worker refuses the owner's cookie on browser requests from another origin. Behind
		// this proxy the page's origin differs from the Worker's, so the proxy names the Worker's.
		proxy: {
			'/api': {
				target: WORKER_DEV_ORIGIN,
				changeOrigin: true,
				headers: { origin: WORKER_DEV_ORIGIN }
			},
			'/ws': {
				target: WORKER_DEV_ORIGIN,
				changeOrigin: true,
				ws: true,
				headers: { origin: WORKER_DEV_ORIGIN }
			}
		}
	},
	test: {
		include: ['src/**/*.test.ts'],
		environment: 'node'
	}
});
