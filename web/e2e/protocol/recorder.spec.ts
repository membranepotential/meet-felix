/**
 * The recorder end to end (docs/recording): in a meeting recorded from the start, the mock SFU's
 * WebSocket adapter streams the host's audio into the `RecorderObject` as Cloudflare's would
 * (about 40 s of the real speech in `web/e2e/audio`); after the end, the transcript job sends
 * the chunks to the real Whisper on Workers AI. Runs only in the `recorder` project
 * (`pnpm test:e2e:recorder`), because it needs the network and `wrangler login`, and each run
 * costs Workers AI neurons.
 */
import { writeFileSync } from 'node:fs';
import {
	createMeeting,
	transcriptStatusOf,
	upgradeStatus,
	type TranscriptStatusWire
} from '../fixtures/api.ts';
import { joinAs } from '../fixtures/meeting.ts';
import { isOp, isPublish, type AdapterView } from '../fixtures/mock-sfu-control.ts';
import { publishOffer } from '../fixtures/negotiation.ts';
import { SNIPPETS, speechPcm } from '../fixtures/speech.ts';
import { expect, test } from '../fixtures/test.ts';

/** Audio per adapter packet. */
const PACKET_MS = 20;
/** How long the adapter streams: over 30 s, so the recorder cuts one chunk at its hard limit. */
const STREAM_MS = 40_000;
/** How long the transcript job may take; Whisper usually needs 10–20 s for this audio. */
const JOB_TIMEOUT_MS = 180_000;

test(
	'a recorded meeting’s audio reaches R2 through the SFU adapter and comes back as a transcript',
	{ tag: '@recorder' },
	async ({ api, owner, rooms, sfu, stack }, testInfo) => {
		// Needs the network, `wrangler login` and Workers AI neurons: only `pnpm test:e2e:recorder`.
		test.skip(process.env.E2E_RECORDER !== '1', 'set E2E_RECORDER=1 (pnpm test:e2e:recorder)');
		const audio = testInfo.outputPath('speech.pcm');
		writeFileSync(audio, speechPcm());
		await sfu.setAdapterAudio(audio);

		const meeting = await createMeeting(api, owner.headers, 'fromStart');
		const host = await joinAs(rooms, meeting, 'Felix', owner.headers);
		await test.step('the host joins and recording starts', () => {
			expect(host.welcome.state.recording?.by).toBe(host.id);
		});
		host.client.send(publishOffer([{ mid: '0', kind: 'audio', name: 'mic' }]));
		await host.client.take('publishAnswer');

		const endpoint =
			await test.step('the recorder creates an adapter for the host’s audio at its /rec endpoint', async () => {
				const published = await sfu.waitForCall(isPublish);
				const created = await sfu.waitForCall(isOp('adapters/websocket/new'), {
					timeoutMs: 10_000
				});
				const origin = stack.baseUrl.replace(/^http/, 'ws');
				expect(created.body.tracks).toEqual([
					{
						location: 'remote',
						sessionId: published.session,
						trackName: published.body.tracks[0]?.trackName,
						endpoint: expect.stringMatching(
							new RegExp(`^${origin}/rec/[0-9a-f]{64}/[0-9a-f]{32}/${host.id}/0$`)
						),
						outputCodec: 'pcm'
					}
				]);
				return created.body.tracks[0]?.endpoint ?? '';
			});
		/** The mock's adapter for the host's audio. */
		const adapter = async (): Promise<AdapterView | undefined> =>
			(await sfu.adapters()).find((a) => a.endpoint === endpoint);

		await test.step('the recorder accepts the adapter’s WebSocket', async () => {
			await expect.poll(async () => (await adapter())?.state).toBe('open');
		});
		await test.step('a wrong secret and an unknown adapter generation are refused', async () => {
			const wrongSecret = endpoint.replace(/\/[0-9a-f]{32}\//, `/${'0'.repeat(32)}/`);
			expect.soft(await upgradeStatus(wrongSecret)).toBe(403);
			expect.soft(await upgradeStatus(endpoint.replace(/\/0$/, '/7'))).toBe(410);
		});
		await test.step(`the adapter streams ${STREAM_MS / 1000} s of speech`, async () => {
			await expect
				.poll(async () => (await adapter())?.packets ?? 0, {
					timeout: STREAM_MS + 20_000,
					intervals: [1_000]
				})
				.toBeGreaterThanOrEqual(STREAM_MS / PACKET_MS);
		});

		host.client.send({ type: 'end' });
		const token = await test.step('End carries the transcript token', async () => {
			const { transcript } = await host.client.take('ended');
			expect(transcript).toMatch(/^[0-9a-f]{32}$/);
			return transcript ?? '';
		});
		const streamedMs = await test.step('ending closes the adapter at the SFU', async () => {
			const close = await sfu.waitForCall(isOp('adapters/websocket/close'), {
				timeoutMs: 10_000
			});
			const closing = await adapter();
			expect(close.body.tracks).toEqual([{ adapterId: closing?.adapterId }]);
			await expect.poll(async () => (await adapter())?.state).toBe('closed');
			return ((await adapter())?.packets ?? 0) * PACKET_MS;
		});

		await test.step('the status API: privacy headers, the meeting, its times, the audio’s length', async () => {
			const response = await api.transcript(token);
			expect(response.status).toBe(200);
			expect.soft(response.headers.get('referrer-policy')).toBe('no-referrer');
			expect.soft(response.headers.get('x-robots-tag')).toBe('noindex');
			expect.soft(response.headers.get('cache-control')).toBe('no-store');
			const first = await transcriptStatusOf(response);
			expect.soft(first.meeting).toBe(meeting);
			expect.soft(first.endedAt).toBeGreaterThanOrEqual(first.startedAt);
			// Every chunk the recorder stored in R2 counts here, in milliseconds of audio.
			expect.soft(first.progress.total).toBeGreaterThanOrEqual(streamedMs - 3_000);
			expect.soft(first.progress.total).toBeLessThanOrEqual(streamedMs + 3_000);
		});
		await test.step('an unknown token is 404 with the same headers', async () => {
			const unknown = await api.transcript('f'.repeat(32));
			expect.soft(unknown.status).toBe(404);
			expect.soft(unknown.headers.get('x-robots-tag')).toBe('noindex');
		});

		const last = await test.step('the transcript job finishes', async () => {
			let status: TranscriptStatusWire | undefined;
			const finished = (s: TranscriptStatusWire) =>
				s.status === 'ready' ||
				s.status === 'failed' ||
				(s.status === 'processing' && s.retryAt !== null);
			await expect
				.poll(
					async () => {
						status = await transcriptStatusOf(await api.transcript(token));
						return finished(status);
					},
					{ timeout: JOB_TIMEOUT_MS, intervals: [2_000] }
				)
				.toBe(true);
			if (!status) throw new Error('no status');
			return status;
		});
		await test.step('every stored chunk was handled by the job', () => {
			expect.soft(last.progress.total).toBeGreaterThan(0);
			expect.soft(last.progress.done).toBe(last.progress.total);
		});
		await test.step('/t/{token} is served with no-referrer and noindex (web/static/_headers)', async () => {
			const page = await api.transcriptPage(token);
			expect.soft(page.status).toBe(200);
			expect.soft(page.headers.get('referrer-policy')).toBe('no-referrer');
			expect.soft(page.headers.get('x-robots-tag')).toBe('noindex');
		});
		if (last.status !== 'ready') throw new Error(`the transcript is ${last.status}, not ready`);
		const { transcript } = last;

		await test.step('the transcript names the meeting, the speaker and the consent', () => {
			expect.soft(transcript.meeting).toBe(meeting);
			expect.soft(transcript.speakers.map((s) => s.label)).toEqual(['Felix (host)']);
			expect
				.soft(transcript.consents)
				.toEqual([expect.objectContaining({ name: 'Felix', noticeVersion: 1, withdrawnAt: null })]);
		});
		await test.step('no audio is missing or untranscribed', () => {
			expect.soft(transcript.gaps).toEqual([]);
		});
		await test.step('the transcript has every snippet’s keywords', async () => {
			const text = transcript.paragraphs
				.map((p) => p.text)
				.join(' ')
				.toLowerCase();
			await testInfo.attach('transcript.txt', { body: text, contentType: 'text/plain' });
			console.log(`Whisper: ${text}`);
			const missing = SNIPPETS.flatMap((s) => s.keywords.filter((k) => !text.includes(k)));
			expect.soft(missing, text).toEqual([]);
		});
		await test.step('the downloads', async () => {
			const md = await api.transcriptFile(token, 'transcript.md');
			expect.soft(md.status).toBe(200);
			expect.soft(md.headers.get('content-type')).toMatch(/^text\/markdown/);
			expect
				.soft(md.headers.get('content-disposition'))
				.toBe(`attachment; filename="${meeting}.md"`);
			expect.soft(await md.text()).toMatch(new RegExp(`^# Transcript: ${meeting}`));
			const json = await api.transcriptFile(token, 'transcript.json');
			expect.soft(json.status).toBe(200);
			expect.soft(((await json.json()) as { meeting: string }).meeting).toBe(meeting);
		});
	}
);
