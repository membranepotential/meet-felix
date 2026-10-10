/**
 * Recovery in the browser (D33): the SFU loses a session, the browser reports a failed peer
 * connection, and the room's socket drops. In each case the participant stays in the meeting;
 * while the socket is down, the others see them reconnecting (D35).
 */
import type { WebSocketRoute } from '@playwright/test';
import { createMeeting } from '../fixtures/api.ts';
import { OWNER_NAME, startCall } from '../fixtures/call.ts';
import type { Api, OwnerSession } from '../fixtures/api.ts';
import { isOp, type MockSfu, type SfuCall } from '../fixtures/mock-sfu-control.ts';
import type { Participant } from '../fixtures/participant.ts';
import { expect, test } from '../fixtures/test.ts';

const isNewSession = isOp('new');
type Renegotiation = Extract<SfuCall, { op: 'renegotiate' }>;
const isRenegotiation = isOp('renegotiate');

/** The SFU session the participant's newest subscribe answer went to, once it got there. */
async function answeredSession(who: Participant, sfu: MockSfu): Promise<string | null> {
	const sdp = who.sent('subscribeAnswer').at(-1)?.sdp;
	const call = (await sfu.calls()).find(
		(c): c is Renegotiation => isRenegotiation(c) && c.body.sessionDescription.sdp === sdp
	);
	return call?.session ?? null;
}

/** Whether the participant's newest subscribe answer reached a session other than `old`. */
const answeredOnNewSession =
	(who: Participant, sfu: MockSfu, old: string | null) => async (): Promise<boolean> => {
		const session = await answeredSession(who, sfu);
		return session !== null && session !== old;
	};

/** A call with host and guest, each showing the other's camera, so no pull is pending. */
async function settledCall({
	api,
	owner,
	host,
	guest
}: {
	api: Api;
	owner: OwnerSession;
	host: Participant;
	guest: Participant;
}): Promise<void> {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await startCall(meeting, host, [{ who: guest, name: 'Guest' }]);
	await expect(guest.tile(OWNER_NAME).locator('video')).toBeAttached();
	await expect(host.tile('Guest').locator('video')).toBeAttached();
}

test('the SFU loses the guest’s subscribe session: the room replaces it, the guest negotiates anew', async ({
	api,
	owner,
	host,
	guest,
	sfu
}) => {
	await settledCall({ api, owner, host, guest });
	const subscribe = await answeredSession(guest, sfu);
	if (subscribe === null) throw new Error('the guest never answered a subscribe offer');

	// The SFU loses the guest's session; the host's camera off and on makes the room use it.
	const mark = await sfu.lastSeq();
	await sfu.loseSession(subscribe);
	await host.control('Stop video').click();
	await host.control('Start video').click();

	const lost = await sfu.waitForCall((c) => c.session === subscribe, {
		after: mark,
		timeoutMs: 15_000
	});
	await guest.waitForReceived('subscribeReset', (m) => m.epoch === 1);
	await sfu.waitForCall(isNewSession, { after: lost.seq });
	await expect(guest.tile(OWNER_NAME).locator('video')).toBeAttached({ timeout: 15_000 });
	await expect.poll(answeredOnNewSession(guest, sfu, subscribe), { timeout: 15_000 }).toBe(true);
});

test('a failed peer connection: the client asks for a new session and says it reconnects', async ({
	api,
	owner,
	host,
	guest,
	sfu
}) => {
	await settledCall({ api, owner, host, guest });
	const before = await answeredSession(guest, sfu);

	const mark = await sfu.lastSeq();
	await guest.failSubscribeConnection();
	await expect(
		guest.page.locator('.pill', { hasText: 'Reconnecting to the others…' })
	).toBeVisible();
	await guest.waitForSent('resetSession', (m) => m.kind === 'subscribe' && m.epoch === 0);
	await sfu.waitForCall(isNewSession, { after: mark });
	await guest.waitForReceived('subscribeReset', (m) => m.epoch === 1);

	// The new subscribe connection gets the host's tracks on the new session.
	await expect(guest.tile(OWNER_NAME).locator('video')).toBeAttached({ timeout: 15_000 });
	await expect.poll(answeredOnNewSession(guest, sfu, before), { timeout: 15_000 }).toBe(true);
});

test('a dropped socket shows "Reconnecting…" on both sides and the same participant comes back', async ({
	api,
	owner,
	host,
	guest
}) => {
	// The guest's sockets go through this route: the first connects, then the test drops it,
	// refuses the next attempt, and holds the one after until it lets it through.
	let attempts = 0;
	let first: { page: WebSocketRoute; server: WebSocketRoute } | null = null;
	let release = () => {};
	const released = new Promise<void>((resolve) => (release = resolve));
	await guest.page.routeWebSocket(/\/ws\//, async (socket) => {
		attempts += 1;
		if (attempts === 1) {
			first = { page: socket, server: socket.connectToServer() };
		} else if (attempts === 2) {
			await socket.close({ code: 1011, reason: 'e2e: refused' });
		} else {
			await released;
			socket.connectToServer();
		}
	});
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await startCall(meeting, host, [{ who: guest, name: 'Guest' }]);
	const you = (await guest.waitForReceived('welcome')).you;

	const dropped = first as { page: WebSocketRoute; server: WebSocketRoute } | null;
	if (!dropped) throw new Error('the guest never opened a socket');
	// The server side closes from inside the browser, where `WebSocket.close` accepts only 1000
	// and 3000–4999: with 1001 the room saw no close until the page closed (2026-10-10).
	await dropped.server.close({ code: 4000, reason: 'e2e: dropped' });
	await dropped.page.close({ code: 1001, reason: 'e2e: dropped' });
	const pill = guest.page.locator('.pill', { hasText: 'Reconnecting…' });
	await expect(pill).toBeVisible();
	// The host keeps the guest's tile, marked, while the room waits for the guest.
	const guestTile = host.tile('Guest');
	await expect(guestTile).toContainText('Reconnecting…');
	await expect(guestTile).toHaveClass(/reconnecting/);

	release();
	await expect(pill).toBeHidden();
	await expect(guestTile).not.toContainText('Reconnecting…');
	await expect(guestTile).not.toHaveClass(/reconnecting/);
	const rejoin = await guest.waitForSent('join', (m) => m.resume !== undefined);
	expect(rejoin.resume).toBe(guest.received('welcome')[0]?.resume);
	await expect.poll(() => guest.received('welcome').map((w) => w.you)).toEqual([you, you]);
	await expect(host.tile('Guest')).toBeVisible();
	await expect(guest.tile(OWNER_NAME)).toBeVisible();
});
