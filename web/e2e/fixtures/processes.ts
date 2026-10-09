/**
 * Child processes for the stack: each runs in its own process group, and is stopped by
 * signalling that group by its PID (never by matching command lines), so `wrangler dev` and the
 * `workerd` it starts go together and nothing else is touched.
 */
import { spawn } from 'node:child_process';
import { openSync } from 'node:fs';
import { readFile } from 'node:fs/promises';

export interface Child {
	readonly pid: number;
	/** Resolves with the exit code (or signal) once the process has exited. */
	readonly exited: Promise<string>;
	/** SIGTERM to the group, SIGKILL after `graceMs`; resolves once the process exited. */
	stop(graceMs?: number): Promise<void>;
}

/** Groups still running; killed if the test runner's process exits without stopping them. */
const running = new Set<number>();
process.on('exit', () => {
	for (const pid of running) signalGroup(pid, 'SIGKILL');
});

function signalGroup(pid: number, signal: NodeJS.Signals): void {
	try {
		process.kill(-pid, signal);
	} catch {
		// The group is gone already.
	}
}

/** Starts `command` in a new process group, with stdout and stderr appended to `log`. */
export function spawnLogged(
	command: string,
	args: readonly string[],
	options: { cwd: string; env: NodeJS.ProcessEnv; log: string }
): Child {
	const out = openSync(options.log, 'a');
	const child = spawn(command, args, {
		cwd: options.cwd,
		env: options.env,
		detached: true,
		stdio: ['ignore', out, out]
	});
	const { pid } = child;
	if (pid === undefined) throw new Error(`could not start ${command}`);
	running.add(pid);
	const exited = new Promise<string>((resolve) => {
		child.once('exit', (code, signal) => {
			running.delete(pid);
			resolve(signal ?? String(code));
		});
	});
	let alive = true;
	void exited.then(() => (alive = false));
	return {
		pid,
		exited,
		async stop(graceMs = 5_000) {
			if (!alive) return;
			signalGroup(pid, 'SIGTERM');
			const timer = setTimeout(() => signalGroup(pid, 'SIGKILL'), graceMs);
			await exited;
			clearTimeout(timer);
			// workerd may outlive wrangler for a moment; take the rest of the group along.
			signalGroup(pid, 'SIGKILL');
		}
	};
}

/** Runs `command` to completion with inherited output; throws on a non-zero exit. */
export function run(
	command: string,
	args: readonly string[],
	options: { cwd: string; env?: NodeJS.ProcessEnv }
): Promise<void> {
	return new Promise((resolve, reject) => {
		const child = spawn(command, args, {
			cwd: options.cwd,
			env: options.env ?? process.env,
			stdio: 'inherit'
		});
		child.once('error', reject);
		child.once('exit', (code) =>
			code === 0 ? resolve() : reject(new Error(`${command} ${args.join(' ')} exited with ${code}`))
		);
	});
}

/**
 * Resolves with the first match of `pattern` in the log file at `log`, polled every 100 ms.
 * Fails after `timeoutMs`, as soon as `exited` resolves, or once a line matches `failure`
 * (with that line), for a process that reports an error and keeps running.
 */
export async function waitForLog(
	log: string,
	pattern: RegExp,
	options: { what: string; timeoutMs: number; exited: Promise<string>; failure?: RegExp }
): Promise<RegExpExecArray> {
	let match: RegExpExecArray | null = null;
	await waitUntil(
		`${options.what} (log: ${log})`,
		async () => {
			const text = await readFile(log, 'utf8');
			const failed = options.failure?.exec(text);
			if (failed) throw new FatalError(`${options.what} failed: ${failed[0]} (log: ${log})`);
			match = pattern.exec(text);
			return match !== null;
		},
		{ timeoutMs: options.timeoutMs, abort: options.exited }
	);
	if (match === null) throw new Error(`${options.what}: no match in ${log}`);
	return match;
}

/** A failure that `waitUntil` must not retry; other errors from `ready` count as "not yet". */
export class FatalError extends Error {}

const fatal = (error: unknown): false => {
	if (error instanceof FatalError) throw error;
	return false;
};

/** Polls `ready` every 200 ms until it resolves `true`; fails after `timeoutMs`, or as soon as `abort` resolves (the process exited). */
export async function waitUntil(
	what: string,
	ready: () => Promise<boolean>,
	options: { timeoutMs: number; abort?: Promise<string> }
): Promise<void> {
	let aborted: string | null = null;
	void options.abort?.then((reason) => (aborted = reason));
	const deadline = Date.now() + options.timeoutMs;
	while (Date.now() < deadline) {
		if (aborted !== null) throw new Error(`${what}: the process exited (${aborted})`);
		if (await ready().catch((error: unknown) => fatal(error))) return;
		await new Promise((resolve) => setTimeout(resolve, 200));
	}
	throw new Error(`${what}: not ready after ${options.timeoutMs} ms`);
}
