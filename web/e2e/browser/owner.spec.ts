/**
 * The owner's pages: login on `/`, creating a meeting, past meetings, logout; and the owner
 * becoming the host from inside a meeting they joined as a guest.
 */
import { createMeeting } from '../fixtures/api.ts';
import { startCall } from '../fixtures/call.ts';
import { expect, test } from '../fixtures/test.ts';

test('the owner logs in, creates a meeting and logs out', async ({ people, stack }) => {
	const owner = await people.open('owner');
	const { page } = owner;
	await page.goto('/');
	await expect(page.getByRole('heading', { name: 'Log in' })).toBeVisible();

	await page.getByLabel('Password').fill('not-the-password');
	await page.getByRole('button', { name: 'Log in' }).click();
	await expect(page.getByRole('alert')).toHaveText('That password is wrong.');

	await page.getByLabel('Password').fill(stack.ownerPassword);
	await page.getByRole('button', { name: 'Log in' }).click();
	await expect(page.getByRole('heading', { name: 'New meeting' })).toBeVisible();
	await expect(page.getByRole('switch')).toBeChecked();

	await page.getByRole('button', { name: 'New meeting' }).click();
	const joinNow = page.getByRole('link', { name: 'Join now' });
	await expect(joinNow).toHaveAttribute('href', /^\/[a-z]+-[a-z]+-[a-z]+$/);

	await page.goto('/');
	await page.getByRole('button', { name: 'Log out' }).click();
	await expect(page.getByRole('heading', { name: 'Log in' })).toBeVisible();
	expect(await page.evaluate(async () => (await fetch('/api/me')).status)).toBe(401);
});

test('past meetings show each meeting’s phase and the transcript link once ended', async ({
	api,
	host,
	guest,
	owner
}) => {
	const scheduled = await createMeeting(api, owner.headers, 'fromStart');
	const meeting = await createMeeting(api, owner.headers, 'fromStart');
	await startCall(meeting, host, [{ who: guest, name: 'Guest' }]);

	const list = await host.context.newPage();
	const item = (words: string) => list.locator('li.item', { hasText: words });
	await list.goto('/meetings');
	await expect(item(scheduled).locator('.state')).toHaveText('Scheduled');
	await expect(item(meeting).locator('.state')).toHaveText('Live');

	await host.endForAll();
	await expect(host.page.getByRole('heading', { name: 'The meeting has ended' })).toBeVisible();
	await expect
		.poll(async () => {
			await list.reload();
			return item(meeting).locator('.state').textContent();
		})
		.toBe('Ended');
	await expect(item(meeting).getByRole('link', { name: 'Transcript' })).toHaveAttribute(
		'href',
		/^\/t\/[0-9a-f]{32}$/
	);
});

test('join as host inside a live meeting: the owner, joined as a guest, logs in and keeps their place', async ({
	api,
	owner,
	host,
	guest,
	stack
}) => {
	// The owner created the meeting and hosts it; then joins again from a browser without the
	// session cookie (a phone), so the room sees a guest.
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await startCall(meeting, host, [{ who: guest, name: 'Phone' }]);
	const you = (await guest.waitForReceived('welcome')).you;
	const peerConnections = () => guest.page.evaluate(() => window.__e2ePeerConnections?.length);
	const connectionsBefore = await peerConnections();
	await expect(guest.control('End for all')).toHaveCount(0);
	await expect(host.control('Join as host')).toHaveCount(0);

	const since = Date.now();
	await guest.control('Join as host').click();
	const dialog = guest.page.getByRole('dialog', { name: 'Join as host' });
	await dialog.getByLabel('Password').fill(stack.ownerPassword);
	await dialog.getByRole('button', { name: 'Log in' }).click();
	await expect(dialog).toBeHidden();

	// A desktop viewport: the host controls sit in the bar, unfolded.
	await expect(guest.control('End for all')).toBeVisible();
	await expect(guest.control('Join as host')).toHaveCount(0);
	// The same participant on a new socket: no second welcome under another id, no new SFU
	// sessions, and the host never saw the guest leave. The name stays the one typed.
	await expect.poll(() => guest.received('welcome').map((w) => w.you)).toEqual([you, you]);
	expect(guest.sockets).toBe(2);
	expect(await peerConnections()).toBe(connectionsBefore);
	await host.waitForReceived('state', (m) =>
		m.state.participants.some((p) => p.id === you && p.role === 'host')
	);
	const hostSaw = host.frames.flatMap((f) =>
		f.at >= since && f.dir === 'received' && f.msg.type === 'state' ? [f.msg.state] : []
	);
	expect(hostSaw.every((state) => state.participants.some((p) => p.id === you))).toBe(true);
	await expect(guest.tile('Phone (you)')).toBeVisible();
	await expect(host.tile('Phone')).toBeVisible();
	for (const p of [host, guest]) await expect(p.tiles).toHaveCount(2);
});
