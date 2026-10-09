/**
 * A real call through the Cloudflare Realtime SFU (`pnpm test:e2e:real`): needs the SFU app's
 * secrets in `.dev.vars` and network, and is skipped without them. Three people with fake
 * cameras and microphones; every check reads what the browsers actually receive.
 */
import { createMeeting } from '../fixtures/api.ts';
import { OWNER_NAME, startCall } from '../fixtures/call.ts';
import { growingOver, inbound, tileTrackId, tileVideoWidth } from '../fixtures/media-stats.ts';
import type { Participant } from '../fixtures/participant.ts';
import { hasRealSfuSecrets } from '../fixtures/stack.ts';
import { expect, test } from '../fixtures/test.ts';

test.describe('real SFU', { tag: '@real' }, () => {
	test.skip(
		!hasRealSfuSecrets(),
		'needs CREATE_SECRET, CALLS_APP_ID and CALLS_APP_SECRET in .dev.vars'
	);

	test('a three-party call: media both ways, a screen share, a layer change, and a 60 s hold', async ({
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

		await test.step('a layer change: a small window gets a smaller layer', async () => {
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
});
