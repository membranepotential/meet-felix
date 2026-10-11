/**
 * The local stack the e2e tests run against: `wrangler dev` with the Worker from the build in
 * `.artifacts/worker`, and one of three kinds (`StackKind`): the mock SFU offline, the mock SFU
 * with the real Workers AI, or the real Cloudflare Realtime SFU.
 *
 * Each stack gets its own directory under `.artifacts/stacks/`: a copy of `wrangler.toml`
 * without the `[build]` table (so `wrangler dev` neither rebuilds nor watches the crate while
 * tests run), its Durable Object and KV state, and the logs of both processes.
 *
 * Every test talks to the Worker from one address, so the copy raises the per-client rate
 * limits out of reach (`RateLimits`), except for a stack that tests them.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer, type AddressInfo } from 'node:net';
import { join } from 'node:path';
import { MockSfu } from './mock-sfu-control.ts';
import {
	DEV_VARS,
	MOCK_SFU_SCRIPT,
	STACKS_DIR,
	WEB_BUILD_DIR,
	WEB_DIR,
	WORKER_BUILD_DIR,
	WRANGLER_BIN,
	WRANGLER_CONFIG
} from './paths.ts';
import { spawnLogged, waitForLog, waitUntil, type Child } from './processes.ts';

/**
 * - `mock`: the mock SFU, and no remote bindings, so no network and no Cloudflare login; the
 *   transcript job cannot reach Workers AI.
 * - `recorder`: the mock SFU, with Workers AI remote as `wrangler dev` runs it by default: needs
 *   the network and `wrangler login`, and each transcript costs Workers AI neurons.
 * - `real`: the real Realtime SFU with the secrets from the repo's `.dev.vars`.
 */
export type StackKind = 'mock' | 'recorder' | 'real';

/**
 * - `relaxed`: every `[[ratelimits]]` limit raised to `RELAXED_LIMIT`.
 * - `deployed`: the limits as `wrangler.toml` sets them.
 */
export type RateLimits = 'relaxed' | 'deployed';

/** A per-minute limit that no test run reaches. */
const RELAXED_LIMIT = 1_000_000;

/**
 * The owner password (`CREATE_SECRET`) of the mock and recorder stacks; the Worker refuses one
 * shorter than 20 characters.
 */
const E2E_OWNER_PASSWORD = 'e2e-owner-password-0123456789';

/**
 * The `SESSION_KEY` of every stack, unless `.dev.vars` sets one for a `real` stack; the Worker
 * refuses one shorter than 32 characters. A test value, never a deployment's.
 */
const E2E_SESSION_KEY = 'e2e-session-key-0123456789abcdefghij';

export interface Stack {
	kind: StackKind;
	/** The Worker, e.g. `http://localhost:41234`. */
	baseUrl: string;
	/** The owner password (`CREATE_SECRET`); also accepted as a bearer token. */
	ownerPassword: string;
	/** The mock SFU's control client; `null` against the real SFU. */
	sfu: MockSfu | null;
	/** This stack's directory with its logs. */
	dir: string;
	stop(): Promise<void>;
}

/** The owner's display name (`OWNER_NAME`) in every stack; invented, like all test names. */
export const OWNER_NAME = 'Robin Hart';

/** The secrets a `real` stack needs from `.dev.vars`. */
const REAL_SECRETS = ['CREATE_SECRET', 'CALLS_APP_ID', 'CALLS_APP_SECRET'] as const;

/** `KEY=value` lines of a `.dev.vars` file; quotes around a value are dropped. */
function parseDevVars(text: string): Map<string, string> {
	return new Map(
		text.split('\n').flatMap((line) => {
			const match = /^\s*([A-Z0-9_]+)\s*=\s*(.*?)\s*$/.exec(line);
			return match?.[1] && match[2] !== undefined
				? [[match[1], match[2].replace(/^"(.*)"$/, '$1')] as const]
				: [];
		})
	);
}

/** Whether `.dev.vars` holds what a `real` stack needs. Never returns or logs the values. */
export function hasRealSfuSecrets(): boolean {
	if (!existsSync(DEV_VARS)) return false;
	const vars = parseDevVars(readFileSync(DEV_VARS, 'utf8'));
	return REAL_SECRETS.every((key) => (vars.get(key) ?? '') !== '');
}

/**
 * `wrangler.toml` for a prebuilt Worker: drops the `[build]` table, points `main` and the
 * assets directory at absolute paths, so the copy works from any directory, and with
 * `relaxed` raises each `[[ratelimits]]` limit (written as `simple = { limit = N, ... }`).
 * TOML keys belong to the table header above them, so a line-wise walk over the headers is
 * exact for this file.
 */
export function prebuiltConfig(
	toml: string,
	main: string,
	assets: string,
	rateLimits: RateLimits
): string {
	let table = '';
	const replaced = new Set<string>();
	const lines = toml.split('\n').flatMap((line) => {
		const header = /^\s*\[\[?\s*([^\]]+?)\s*\]\]?\s*$/.exec(line);
		if (header?.[1] !== undefined) table = header[1];
		if (table === 'build') return [];
		if (table === '' && /^\s*main\s*=/.test(line)) {
			replaced.add('main');
			return [`main = ${JSON.stringify(main)}`];
		}
		if (table === 'assets' && /^\s*directory\s*=/.test(line)) {
			replaced.add('assets');
			return [`directory = ${JSON.stringify(assets)}`];
		}
		const limit = /^(\s*simple\s*=\s*\{\s*limit\s*=\s*)[\d_]+/;
		if (table === 'ratelimits' && rateLimits === 'relaxed' && limit.test(line)) {
			replaced.add('ratelimits');
			return [line.replace(limit, `$1${RELAXED_LIMIT}`)];
		}
		return [line];
	});
	if (!replaced.has('main') || !replaced.has('assets')) {
		throw new Error('wrangler.toml has no top-level `main` or no `[assets] directory`');
	}
	if (rateLimits === 'relaxed' && !replaced.has('ratelimits')) {
		throw new Error('wrangler.toml has no `[[ratelimits]]` with `simple = { limit = N, ... }`');
	}
	return lines.join('\n');
}

/**
 * A port that was free a moment ago. The race with another process taking it in between is
 * accepted: `wrangler dev` then fails with an `[ERROR]` line, which fails the stack loudly.
 */
async function freePort(): Promise<number> {
	const server = createServer();
	await new Promise<void>((resolve) => server.listen(0, 'localhost', resolve));
	const { port } = server.address() as AddressInfo;
	await new Promise((resolve) => server.close(resolve));
	return port;
}

async function startMockSfu(dir: string, appId: string, appSecret: string) {
	const log = join(dir, 'mock-sfu.log');
	const child = spawnLogged(process.execPath, [MOCK_SFU_SCRIPT, '0', appId, appSecret], {
		cwd: WEB_DIR,
		env: process.env,
		log
	});
	const [, url = ''] = await waitForLog(log, /listening on (http:\/\/\S+)/, {
		what: 'mock SFU',
		timeoutMs: 10_000,
		exited: child.exited
	});
	return { child, url };
}

/**
 * Starts a stack named `name` (used for its directory); stop it with `stop()`. Its rate limits
 * are `relaxed` unless a test of the limits asks for the `deployed` ones.
 */
export async function startStack(
	kind: StackKind,
	name: string,
	rateLimits: RateLimits = 'relaxed'
): Promise<Stack> {
	for (const [what, path] of [
		['the Worker bundle', join(WORKER_BUILD_DIR, 'index.js')],
		['the web build', join(WEB_BUILD_DIR, 'index.html')]
	] as const) {
		if (!existsSync(path)) {
			throw new Error(`${what} is missing (${path}); run the e2e tests without E2E_SKIP_BUILD`);
		}
	}
	const dir = join(STACKS_DIR, `${name}-${Date.now()}`);
	mkdirSync(dir, { recursive: true });
	const children: Child[] = [];
	const stop = async () => {
		for (const child of children.reverse()) await child.stop();
	};
	try {
		let ownerPassword: string;
		let sfu: MockSfu | null = null;
		// The Worker's port: 0 lets wrangler pick one and log it, so stacks started side by side
		// never race for a port. Against the mock SFU, the recorder's `RECORDER_ORIGIN` must name
		// the Worker itself (the mock's adapters connect there), so the port is chosen up front.
		let port = 0;
		const flags = ['--var', 'ROOM_JURISDICTION:', '--var', `OWNER_NAME:${OWNER_NAME}`];
		if (kind === 'mock' || kind === 'recorder') {
			// `--local` disables the remote bindings; Workers AI is the only one this Worker has.
			if (kind === 'mock') flags.push('--local');
			ownerPassword = E2E_OWNER_PASSWORD;
			const [appId, appSecret] = ['e2e', 'e2e-sfu-secret'];
			const mock = await startMockSfu(dir, appId, appSecret);
			children.push(mock.child);
			sfu = new MockSfu(mock.url);
			// Picked right before wrangler starts, so no other process of this run takes it first.
			port = await freePort();
			flags.push(
				...[
					`CREATE_SECRET:${ownerPassword}`,
					`SESSION_KEY:${E2E_SESSION_KEY}`,
					`CALLS_APP_ID:${appId}`,
					`CALLS_APP_SECRET:${appSecret}`,
					`SFU_API_URL:${mock.url}/v1`,
					`RECORDER_ORIGIN:http://localhost:${port}`
				].flatMap((v) => ['--var', v])
			);
		} else {
			// Leaves wrangler.toml's placeholder `RECORDER_ORIGIN` as it is: the @real specs never
			// record (the real SFU cannot reach localhost; that would need `wrangler dev --tunnel`).
			if (!hasRealSfuSecrets()) throw new Error(`${DEV_VARS} lacks ${REAL_SECRETS.join(', ')}`);
			const devVars = parseDevVars(readFileSync(DEV_VARS, 'utf8'));
			ownerPassword = devVars.get('CREATE_SECRET') ?? '';
			// Read in place: the secrets are never copied into the stack's directory.
			flags.push('--env-file', DEV_VARS);
			if ((devVars.get('SESSION_KEY') ?? '') === '') {
				flags.push('--var', `SESSION_KEY:${E2E_SESSION_KEY}`);
			}
		}
		const config = join(dir, 'wrangler.toml');
		writeFileSync(
			config,
			prebuiltConfig(
				readFileSync(WRANGLER_CONFIG, 'utf8'),
				join(WORKER_BUILD_DIR, 'index.js'),
				WEB_BUILD_DIR,
				rateLimits
			)
		);
		const log = join(dir, 'wrangler.log');
		const wrangler = spawnLogged(
			WRANGLER_BIN,
			[
				'dev',
				'--config',
				config,
				'--port',
				String(port),
				'--inspector-port',
				'0',
				'--persist-to',
				join(dir, 'state'),
				'--show-interactive-dev-session=false',
				...flags
			],
			{
				cwd: dir,
				env: { ...process.env, WRANGLER_SEND_METRICS: 'false', FORCE_COLOR: '0' },
				log
			}
		);
		children.push(wrangler);
		const [, baseUrl = ''] = await waitForLog(log, /Ready on (http:\/\/\S+)/, {
			what: 'wrangler dev',
			timeoutMs: 90_000,
			exited: wrangler.exited,
			failure: /\[ERROR\].*/
		});
		// The Worker itself answers: without a session cookie, `/api/me` is 401.
		await waitUntil('the Worker', async () => (await fetch(`${baseUrl}/api/me`)).status === 401, {
			timeoutMs: 30_000,
			abort: wrangler.exited
		});
		return { kind, baseUrl, ownerPassword, sfu, dir, stop };
	} catch (error) {
		await stop();
		throw error;
	}
}
