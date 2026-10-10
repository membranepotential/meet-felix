/** Where the e2e run finds the builds and keeps its own files. */
import { fileURLToPath } from 'node:url';

const at = (relative: string) => fileURLToPath(new URL(relative, import.meta.url));

export const REPO_DIR = at('../../../');
export const WEB_DIR = at('../../');
/** The SvelteKit build that `wrangler dev` serves as static assets. */
export const WEB_BUILD_DIR = at('../../build/');
export const WORKER_CRATE_DIR = at('../../../crates/worker/');
export const WRANGLER_CONFIG = at('../../../wrangler.toml');
export const WRANGLER_BIN = at('../../../node_modules/.bin/wrangler');
export const DEV_VARS = at('../../../.dev.vars');
export const MOCK_SFU_SCRIPT = at('../mock-sfu.ts');
/** Real speech for the recorder test; see the README there. */
export const AUDIO_DIR = at('../audio/');

/** Git-ignored: test results, the Worker bundle, and one directory per running stack. */
export const ARTIFACTS_DIR = at('../.artifacts/');
export const WORKER_BUILD_DIR = at('../.artifacts/worker/');
export const STACKS_DIR = at('../.artifacts/stacks/');
