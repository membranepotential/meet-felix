/**
 * The local stack the e2e tests run against: `wrangler dev` with the Worker from the build in
 * `.artifacts/worker`, and either the mock SFU (`mock`) or the real Cloudflare Realtime SFU
 * (`real`, with the secrets from the repo's `.dev.vars`).
 *
 * Each stack gets its own directory under `.artifacts/stacks/`: a copy of `wrangler.toml`
 * without the `[build]` table (so `wrangler dev` neither rebuilds nor watches the crate while
 * tests run), its Durable Object and KV state, and the logs of both processes.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
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

export type StackKind = 'mock' | 'real';

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
 * `wrangler.toml` for a prebuilt Worker: drops the `[build]` table and points `main` and the
 * assets directory at absolute paths, so the copy works from any directory. TOML keys belong to
 * the table header above them, so a line-wise walk over the headers is exact for this file.
 */
export function prebuiltConfig(toml: string, main: string, assets: string): string {
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
		return [line];
	});
	if (!replaced.has('main') || !replaced.has('assets')) {
		throw new Error('wrangler.toml has no top-level `main` or no `[assets] directory`');
	}
	return lines.join('\n');
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

/** Starts a stack named `name` (used for its directory); stop it with `stop()`. */
export async function startStack(kind: StackKind, name: string): Promise<Stack> {
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
		const flags = ['--var', 'ROOM_JURISDICTION:'];
		if (kind === 'mock') {
			// No remote bindings (Workers AI runs remotely in `wrangler dev`): the local stack needs
			// no network and no Cloudflare login.
			flags.push('--local');
			ownerPassword = 'e2e-owner-password';
			const [appId, appSecret] = ['e2e', 'e2e-sfu-secret'];
			const mock = await startMockSfu(dir, appId, appSecret);
			children.push(mock.child);
			sfu = new MockSfu(mock.url);
			flags.push(
				...[
					`CREATE_SECRET:${ownerPassword}`,
					`CALLS_APP_ID:${appId}`,
					`CALLS_APP_SECRET:${appSecret}`,
					`SFU_API_URL:${mock.url}/v1`
				].flatMap((v) => ['--var', v])
			);
		} else {
			if (!hasRealSfuSecrets()) throw new Error(`${DEV_VARS} lacks ${REAL_SECRETS.join(', ')}`);
			ownerPassword = parseDevVars(readFileSync(DEV_VARS, 'utf8')).get('CREATE_SECRET') ?? '';
			// Read in place: the secrets are never copied into the stack's directory.
			flags.push('--env-file', DEV_VARS);
		}
		const config = join(dir, 'wrangler.toml');
		writeFileSync(
			config,
			prebuiltConfig(
				readFileSync(WRANGLER_CONFIG, 'utf8'),
				join(WORKER_BUILD_DIR, 'index.js'),
				WEB_BUILD_DIR
			)
		);
		// Port 0 everywhere: wrangler picks free ports and logs the one it serves on, so stacks
		// started side by side never race for a port.
		const log = join(dir, 'wrangler.log');
		const wrangler = spawnLogged(
			WRANGLER_BIN,
			[
				'dev',
				'--config',
				config,
				'--port',
				'0',
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
