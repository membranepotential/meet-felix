/**
 * Loader for `meeting-core` compiled to WASM.
 *
 * Build it with `wasm-pack build crates/meeting-core --target web --out-dir ../../web/src/lib/wasm`.
 * The `web` target fetches its `.wasm` via `new URL(..., import.meta.url)`, which Vite
 * bundles without plugins. `import.meta.glob` matches nothing while the package is absent,
 * so the app builds and runs without it and `loadCore()` returns `null`.
 */

/** The part of the wasm-pack `web` output we rely on until tsify types are generated. */
interface CoreModule {
	/** wasm-bindgen's init: fetches and instantiates the `.wasm`. */
	default: () => Promise<WebAssembly.Exports>;
}

const packages = import.meta.glob<CoreModule>('./wasm/meeting_core.js');

let loading: Promise<CoreModule | null> | undefined;

export function loadCore(): Promise<CoreModule | null> {
	loading ??= (async () => {
		const load = packages['./wasm/meeting_core.js'];
		if (!load) return null;
		const core = await load();
		await core.default();
		return core;
	})();
	return loading;
}
