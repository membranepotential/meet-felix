/**
 * A real call through the Cloudflare Realtime SFU (`pnpm test:e2e:real`): needs the SFU app's
 * secrets in `.dev.vars` and network, and is skipped without them. Three people with fake
 * cameras and microphones; every check reads what the browsers actually receive.
 */
import { createMeeting } from '../fixtures/api.ts';
import { OWNER_NAME, startCall } from '../fixtures/call.ts';
import {
	decodingVideo,
	growingOver,
	inbound,
	tileTrackId,
	tileVideoWidth
} from '../fixtures/media-stats.ts';
import type { Participant } from '../fixtures/participant.ts';
import { hasRealSfuSecrets } from '../fixtures/stack.ts';
import { expect, test } from '../fixtures/test.ts';

/** How long a subscriber's smaller layer must hold after `SetLayer`; a climb back took ~12 s. */
const LAYER_HOLD_MS = 25_000;
/** The five-party call checks every tile's video in rounds of this length, 30 s in all. */
const ROUND_MS = 10_000;
const ROUNDS = 3;

test.describe('real SFU', { tag: '@real' }, () => {
	test.skip(
		!hasRealSfuSecrets(),
		'needs CREATE_SECRET, CALLS_APP_ID and CALLS_APP_SECRET in .dev.vars'
	);

	test('a three-party call: media both ways, a screen share, a layer change that holds, and a 60 s hold', async ({
		api,
		owner,
		host,
		people
	}) => {
		test.setTimeout(240_000);
		const [ada, bo] = await Promise.all([people.open('ada'), people.open('bo')]);
		const meeting = await createMeeting(api, owner.headers, 'hostDecides');
		await startCall(meeting, host, [
			{ who: ada, name: 'Ada' },
			{ who: bo, name: 'Bo' }
		]);
		const everyone: { who: Participant; others: string[] }[] = [
			{ who: host, others: ['Ada', 'Bo'] },
			{ who: ada, others: [OWNER_NAME, 'Bo'] },
			{ who: bo, others: [OWNER_NAME, 'Ada'] }
		];

		await test.step('everyone receives the others’ video and audio', async () => {
			for (const { who, others } of everyone) {
				for (const name of others) {
					await expect
						.poll(() => tileVideoWidth(who.tile(name)), { timeout: 20_000 })
						.toBeGreaterThan(0);
				}
				const grew = await growingOver(who.page, 3_000);
				const tracks = await Promise.all(others.map((name) => tileTrackId(who.tile(name))));
				for (const track of tracks) {
					expect(grew.some((s) => s.kind === 'video' && s.trackIdentifier === track)).toBe(true);
				}
				expect(grew.filter((s) => s.kind === 'audio')).toHaveLength(others.length);
			}
		});

		await test.step('a layer change: a small window gets a smaller layer, and keeps it', async () => {
			const tile = ada.tile(OWNER_NAME);
			const track = await tileTrackId(tile);
			const hostId = (await ada.waitForReceived('welcome')).state.participants.find(
				(p) => p.role === 'host'
			)?.id;
			// The layer Ada asked for last; the SFU sends the full one (`f`) until she asks.
			const layer = () =>
				ada
					.sent('setLayer')
					.filter((m) => m.track.owner === hostId && m.track.kind === 'video')
					.at(-1)?.rid ?? 'f';
			const width = async () =>
				(await inbound(ada.page)).find((s) => s.trackIdentifier === track)?.frameWidth ?? 0;
			await ada.page.setViewportSize({ width: 1920, height: 1080 });
			await expect.poll(layer).toBe('f');
			await expect.poll(width, { timeout: 20_000 }).toBeGreaterThan(400);
			const large = await width();
			await ada.page.setViewportSize({ width: 400, height: 300 });
			await expect.poll(layer).not.toBe('f');
			// Half (`h`) or a quarter (`q`) of the full layer's width.
			await expect.poll(width, { timeout: 20_000 }).toBeLessThanOrEqual(large / 2);

			// The SFU keeps the smaller layer: no climb back to full without a new `SetLayer`.
			const asked = ada.sent('setLayer').length;
			const samples: { at: number; width: number }[] = [];
			const start = Date.now();
			while (Date.now() - start < LAYER_HOLD_MS) {
				samples.push({ at: Date.now() - start, width: await width() });
				await ada.page.waitForTimeout(1_000);
			}
			// The trace goes to the run's output: a climb back shows when it happened.
			console.log(
				`layer hold (${layer()}, full ${large} px): ${samples.map((s) => `${s.at}ms=${s.width}`).join(' ')}`
			);
			expect(ada.sent('setLayer').length, 'Ada asked for no other layer').toBe(asked);
			expect(
				samples.filter((s) => s.width > large / 2),
				`decoded widths over ${LAYER_HOLD_MS / 1000} s stay at most ${large / 2}`
			).toEqual([]);
			await ada.page.setViewportSize({ width: 1280, height: 800 });
		});

		await test.step('a screen share reaches the others', async () => {
			await host.control('Share screen').click();
			for (const who of [ada, bo]) {
				const screen = who.tile(`${OWNER_NAME}’s screen`);
				await expect.poll(() => tileVideoWidth(screen), { timeout: 20_000 }).toBeGreaterThan(0);
			}
			await host.control('Stop sharing').click();
		});

		await test.step('60 s without reconnects, media still flowing', async () => {
			const before = everyone.map(({ who }) => ({
				sockets: who.sockets,
				resets: who.received('subscribeReset').length + who.received('publishReset').length
			}));
			await host.page.waitForTimeout(60_000);
			for (const [i, { who, others }] of everyone.entries()) {
				expect(who.sockets, `${who.label} opened no new socket`).toBe(before[i]?.sockets);
				expect(
					who.received('subscribeReset').length + who.received('publishReset').length,
					`${who.label} had no session reset`
				).toBe(before[i]?.resets);
				const grew = await growingOver(who.page, 3_000);
				expect(grew.filter((s) => s.kind === 'video').length).toBeGreaterThanOrEqual(others.length);
				expect(grew.filter((s) => s.kind === 'audio')).toHaveLength(others.length);
			}
		});
	});

	test('a five-party call: everyone decodes everyone else’s video for 30 s', async ({
		api,
		owner,
		host,
		people
	}) => {
		test.setTimeout(240_000);
		const names = ['Ada', 'Bo', 'Cy', 'Di'];
		const guests = await Promise.all(names.map((name) => people.open(name.toLowerCase())));
		const meeting = await createMeeting(api, owner.headers, 'hostDecides');
		await startCall(
			meeting,
			host,
			guests.map((who, i) => ({ who, name: names[i] ?? '' }))
		);
		const everyone: { who: Participant; others: string[] }[] = [
			{ who: host, others: names },
			...guests.map((who, i) => ({
				who,
				others: [OWNER_NAME, ...names.filter((_, j) => j !== i)]
			}))
		];
		for (const { who, others } of everyone)
			for (const name of others)
				await expect
					.poll(() => tileVideoWidth(who.tile(name)), { timeout: 30_000 })
					.toBeGreaterThan(0);

		for (let round = 1; round <= ROUNDS; round++) {
			await test.step(`round ${round}: every tile's video grew over ${ROUND_MS / 1000} s`, async () => {
				const tracks = await Promise.all(
					everyone.map(({ who, others }) =>
						Promise.all(others.map((name) => tileTrackId(who.tile(name))))
					)
				);
				const before = await Promise.all(everyone.map(({ who }) => inbound(who.page)));
				await host.page.waitForTimeout(ROUND_MS);
				const after = await Promise.all(everyone.map(({ who }) => inbound(who.page)));
				for (const [i, { who, others }] of everyone.entries()) {
					const decoding = decodingVideo(before[i] ?? [], after[i] ?? []);
					expect(
						others.filter((_, j) => !decoding.has(tracks[i]?.[j] ?? '')),
						`${who.label}: tiles whose video did not grow`
					).toEqual([]);
				}
			});
		}
	});
});
