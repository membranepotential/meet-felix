import { readFileSync } from 'node:fs';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { listMeetings } from './api.ts';
import { initSync } from './wasm/meeting_core.js';

beforeAll(() => {
	initSync({ module: readFileSync(new URL('./wasm/meeting_core_bg.wasm', import.meta.url)) });
});

afterEach(() => {
	vi.unstubAllGlobals();
});

/** Answers every `fetch` with `body` and `status`, and records the requests. */
function serve(status: number, body: unknown) {
	const fetch = vi.fn(async () => new Response(JSON.stringify(body), { status }));
	vi.stubGlobal('fetch', fetch);
	return fetch;
}

describe('listMeetings', () => {
	it('sends the password as a bearer token and lists newest first', async () => {
		const fetch = serve(200, {
			meetings: [
				{ meeting: 'abide-acid-acorn', createdAt: 1, ended: true, transcript: 't1' },
				{ meeting: 'abide-acid-acorn', createdAt: 2, ended: false }
			]
		});
		const result = await listMeetings('secret');
		expect(fetch).toHaveBeenCalledWith('/api/meetings', {
			headers: { Authorization: 'Bearer secret' }
		});
		expect(result).toEqual({
			ok: true,
			value: [
				{ meeting: 'abide-acid-acorn', createdAt: 2, ended: false, transcript: null },
				{ meeting: 'abide-acid-acorn', createdAt: 1, ended: true, transcript: 't1' }
			]
		});
	});

	it('reports a wrong password and a server without the endpoint', async () => {
		serve(401, {});
		expect(await listMeetings('wrong')).toEqual({ ok: false, error: { kind: 'unauthorized' } });
		serve(404, {});
		expect(await listMeetings('secret')).toEqual({
			ok: false,
			error: { kind: 'server', status: 404 }
		});
	});

	it('rejects a list with an entry that is not a meeting', async () => {
		serve(200, { meetings: [{ meeting: 'not a meeting', createdAt: 1, ended: true }] });
		expect(await listMeetings('secret')).toEqual({ ok: false, error: { kind: 'malformed' } });
	});
});
