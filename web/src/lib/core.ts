/**
 * `meeting-core` compiled to WASM (`pnpm build:core` writes it to `./wasm/`, gitignored).
 *
 * The `web` target loads its `.wasm` through `new URL(..., import.meta.url)`, which Vite
 * bundles without a plugin. The import is static on purpose: without the package,
 * `vite.config.ts` stops dev and build with a hint, and `svelte-check` reports the missing
 * module, instead of the app running with protocol types that may be stale or absent.
 */
import init from './wasm/meeting_core.js';

export { generateMeetingId, validateMeetingId } from './wasm/meeting_core.js';

let loading: Promise<void> | undefined;

/** Instantiates the module once; call before any exported function (the root layout does). */
export function loadCore(): Promise<void> {
	loading ??= init().then(() => undefined);
	return loading;
}
