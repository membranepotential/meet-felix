/**
 * Builds what every stack serves, once per run: the web app (WASM core plus SvelteKit, into
 * `web/build`) and the Worker bundle (into `.artifacts/worker`, so a `wrangler dev` started
 * elsewhere never rebuilds it mid-run). With `E2E_SKIP_BUILD=1`, both builds are reused as
 * they are.
 */
import { rmSync } from 'node:fs';
import { delimiter, join } from 'node:path';
import { homedir } from 'node:os';
import { REPO_DIR, STACKS_DIR, WORKER_BUILD_DIR, WORKER_CRATE_DIR } from './fixtures/paths.ts';
import { run } from './fixtures/processes.ts';

export default async function globalSetup(): Promise<void> {
	rmSync(STACKS_DIR, { recursive: true, force: true });
	if (process.env.E2E_SKIP_BUILD === '1') return;
	// worker-build and wasm-pack call cargo; rustup installs it in ~/.cargo/bin.
	const cargoBin = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin');
	const env = { ...process.env, PATH: `${cargoBin}${delimiter}${process.env.PATH ?? ''}` };
	await run('pnpm', ['run', 'build'], { cwd: REPO_DIR, env });
	await run('worker-build', ['--release', '--out-dir', WORKER_BUILD_DIR], {
		cwd: WORKER_CRATE_DIR,
		env
	});
}
