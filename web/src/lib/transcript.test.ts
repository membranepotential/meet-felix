import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	duration,
	getTranscript,
	parseTranscriptStatus,
	timeline,
	transcriptFile,
	type Transcript
} from './transcript.ts';

afterEach(() => {
	vi.unstubAllGlobals();
});

const TOKEN = '0123456789abcdef0123456789abcdef';

/** A `transcript.json` as the recorder writes it. */
const transcript: Transcript = {
	meeting: 'amber-otter-ocean',
	startedAt: 1_000,
	endedAt: 3_601_000,
	speakers: [
		{ who: 0, label: 'Felix (host)', role: 'host' },
		{ who: 3, label: 'Anna', role: 'guest' }
	],
	consents: [
		{ who: 0, name: 'Felix', at: 800, noticeVersion: 1, withdrawnAt: null },
		{ who: 3, name: 'Anna', at: 900, noticeVersion: 1, withdrawnAt: 50_000 }
	],
	gaps: [{ who: 3, start: 10_000, end: 22_000, reason: 'missing' }],
	paragraphs: [
		{ who: 0, label: 'Felix (host)', startMs: 1_000, endMs: 4_000, text: 'Guten Tag.' },
		{ who: 3, label: 'Anna', startMs: 10_000, endMs: 12_000, text: 'Hello.' }
	]
};

function serve(status: number, body?: unknown) {
	const fetch = vi.fn(
		async () => new Response(body === undefined ? null : JSON.stringify(body), { status })
	);
	vi.stubGlobal('fetch', fetch);
	return fetch;
}

describe('transcript status', () => {
	it('reads a ready transcript, including its version field', async () => {
		const fetch = serve(200, {
			status: 'ready',
			meeting: transcript.meeting,
			startedAt: 1_000,
			endedAt: 3_601_000,
			progress: { done: 4, total: 4 },
			audioUntil: 5,
			// As the recorder writes it: `withdrawnAt` is left out while consent stands.
			transcript: {
				version: 1,
				...transcript,
				consents: [{ who: 0, name: 'Felix', at: 800, noticeVersion: 1 }, transcript.consents[1]]
			}
		});
		expect(await getTranscript(TOKEN)).toEqual({
			ok: true,
			value: {
				status: 'ready',
				meeting: transcript.meeting,
				startedAt: 1_000,
				endedAt: 3_601_000,
				transcript,
				audioUntil: 5
			}
		});
		expect(fetch).toHaveBeenCalledWith(`/api/transcript/${TOKEN}`);
	});

	it('reads progress and the time the job continues after the daily quota', () => {
		const body = {
			status: 'processing',
			meeting: 'a-b-c',
			startedAt: 1,
			endedAt: 2,
			progress: { done: 60_000, total: 540_000 }
		};
		expect(parseTranscriptStatus(body)).toEqual({ ...body, retryAt: null });
		const running = {
			status: 'recording',
			meeting: 'a-b-c',
			startedAt: 1,
			progress: body.progress
		};
		expect(parseTranscriptStatus(running)).toEqual({ ...running, endedAt: null });
		expect(parseTranscriptStatus({ ...body, retryAt: 42 })).toEqual({ ...body, retryAt: 42 });
		expect(parseTranscriptStatus({ ...body, retryAt: 'soon' })).toBeNull();
	});

	it('rejects bodies that are not what the Worker promises', () => {
		for (const body of [
			null,
			{ status: 'ready', meeting: 'a-b-c', startedAt: 1 },
			{ status: 'done', meeting: 'a-b-c', startedAt: 1 },
			{ status: 'failed', meeting: 'a-b-c' },
			{ status: 'recording', meeting: 'a-b-c', startedAt: 1, progress: { done: '1', total: 2 } },
			{
				status: 'ready',
				meeting: 'a-b-c',
				startedAt: 1,
				transcript: { ...transcript, gaps: [{ ...transcript.gaps[0], reason: 'lost' }] }
			}
		]) {
			expect(parseTranscriptStatus(body)).toBeNull();
		}
	});

	it('turns 404 into not-found and other failures into errors', async () => {
		serve(404);
		expect(await getTranscript(TOKEN)).toEqual({ ok: true, value: { status: 'not-found' } });
		serve(500);
		expect(await getTranscript(TOKEN)).toEqual({
			ok: false,
			error: { kind: 'server', status: 500 }
		});
		serve(200, { status: 'nope', meeting: 'a-b-c', startedAt: 1 });
		expect(await getTranscript(TOKEN)).toEqual({ ok: false, error: { kind: 'malformed' } });
	});
});

describe('transcript page helpers', () => {
	it('lists gaps between paragraphs in time order, labelled with the speaker', () => {
		expect(timeline(transcript).map((e) => e.kind)).toEqual(['paragraph', 'gap', 'paragraph']);
		const gap = timeline(transcript).find((e) => e.kind === 'gap');
		expect(gap?.kind === 'gap' ? gap.label : null).toBe('Anna');
	});

	it('links the downloads and words durations', () => {
		expect(transcriptFile(TOKEN, 'md')).toBe(`/api/transcript/${TOKEN}/transcript.md`);
		expect([duration(12_000), duration(12 * 60_000), duration(65 * 60_000)]).toEqual([
			'12 s',
			'12 min',
			'1 h 5 min'
		]);
	});
});
