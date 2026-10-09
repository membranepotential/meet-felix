import { loadCore } from '#lib/core.ts';

// Fully client-rendered SPA: adapter-static emits `index.html` as the fallback for every route.
export const ssr = false;
export const prerender = false;

/** Every page may call into `meeting-core`, so the WASM module is ready before any renders. */
export const load = async (): Promise<void> => {
	await loadCore();
};
