/**
 * A call through the UI against the mock SFU, which negotiates but carries no media: who sees
 * whom, the controls and how the room view follows them, switching devices, screen sharing,
 * leaving and ending.
 */
import type { Page } from '@playwright/test';
import { createMeeting } from '../fixtures/api.ts';
import { hostPreJoin, OWNER_NAME, startCall } from '../fixtures/call.ts';
import { expect, test } from '../fixtures/test.ts';

/** The labels of the tracks the page's senders carry, by kind (the publish connection's). */
function sentTrackLabels(page: Page): Promise<Record<string, string>> {
	return page.evaluate(() =>
		Object.fromEntries(
			(window.__e2ePeerConnections ?? [])
				.filter((pc) => pc.connectionState !== 'closed')
				.flatMap((pc) => pc.getSenders())
				.flatMap((s) => (s.track ? [[s.track.kind, s.track.label]] : []))
		)
	);
}

test('the owner’s pre-join knows the host: name prefilled, no password, no "Join as host"', async ({
	api,
	owner,
	host
}) => {
	const meeting = await createMeeting(api, owner.headers);
	await hostPreJoin(host, meeting);
	await expect(host.page.getByLabel('Your name')).toHaveValue(OWNER_NAME);
	await expect(host.page.getByRole('button', { name: 'Join as host' })).toHaveCount(0);
	await expect(host.page.locator('input[type=password]')).toHaveCount(0);
	// The device pickers sit folded under the preview.
	await host.page.getByText('Microphone and camera', { exact: true }).click();
	await expect(host.page.getByRole('combobox', { name: 'Microphone' })).toBeVisible();
	await expect(host.page.getByRole('combobox', { name: 'Camera' })).toBeVisible();
});

test('a guest waits for the host and is let in when the host joins', async ({
	api,
	owner,
	host,
	guest
}) => {
	const meeting = await createMeeting(api, owner.headers, 'fromStart');
	await guest.openPreJoin(meeting, 'Guest');
	await expect(guest.page.getByText('This meeting is recorded')).toBeVisible();
	await expect(guest.page.getByText('The meeting has not started yet')).toBeVisible();
	await expect(guest.page.getByRole('button', { name: 'Join as host' })).toBeVisible();
	await expect(guest.joinButton).toHaveText('Join and accept recording');
	await guest.join();
	await expect(guest.page.getByRole('heading', { name: 'Waiting for the host' })).toBeVisible();

	await hostPreJoin(host, meeting);
	await host.join();
	await expect(host.meeting).toBeVisible();
	await expect(guest.meeting).toBeVisible();

	await expect(host.tile(`${OWNER_NAME} (you)`)).toBeVisible();
	await expect(host.tile('Guest')).toBeVisible();
	await expect(guest.tile('Guest (you)')).toBeVisible();
	await expect(guest.tile(OWNER_NAME)).toBeVisible();
	for (const p of [host, guest]) {
		await expect(p.page.getByRole('status').filter({ hasText: 'Recording' })).toBeVisible();
	}
	// Only the host has the host controls.
	await expect(await host.hostControl('End for all')).toBeVisible();
	await expect(guest.control('End for all')).toHaveCount(0);
});

test('mute and camera: the buttons, the own tile, and the others’ view follow', async ({
	api,
	owner,
	host,
	guest
}) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await startCall(meeting, host, [{ who: guest, name: 'Guest' }]);
	const hostOnGuest = guest.tile(OWNER_NAME);
	// The mock SFU negotiates real transceivers, so the host's camera tile holds a video.
	await expect(hostOnGuest.locator('video')).toBeAttached();

	await host.control('Mute').click();
	await expect(host.control('Unmute')).toHaveAttribute('aria-pressed', 'true');
	await expect(host.tile(`${OWNER_NAME} (you)`).locator('.muted')).toBeVisible();

	await host.control('Stop video').click();
	await expect(host.control('Start video')).toBeVisible();
	await expect(host.tile(`${OWNER_NAME} (you)`).locator('.avatar')).toHaveText('FP');
	// The room's view drops the camera: the guest shows the host's initials instead.
	await guest.waitForReceived('state', (m) =>
		m.state.participants.some((p) => p.role === 'host' && !p.tracks.includes('video'))
	);
	await expect(hostOnGuest.locator('.avatar')).toHaveText('FP');

	await host.control('Start video').click();
	await expect(hostOnGuest.locator('video')).toBeAttached({ timeout: 15_000 });

	// A host mutes a guest from the guest's tile; the room tells the guest's page.
	await host.tile('Guest').hover();
	await host.page.getByRole('button', { name: 'Mute Guest' }).click();
	await guest.waitForReceived('muted');
	await expect(guest.control('Unmute')).toBeVisible();
});

test('screen share: a featured tile that expands, for the sharer and the others', async ({
	api,
	owner,
	host,
	guest,
	browserName
}) => {
	test.skip(browserName !== 'chromium', 'fake screen capture exists in Chromium only');
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await startCall(meeting, host, [{ who: guest, name: 'Guest' }]);

	await host.control('Share screen').click();
	await expect(host.control('Stop sharing')).toBeVisible();
	await expect(
		host.tile('Your screen').getByRole('button', { name: 'Expand screen' })
	).toBeVisible();
	await host.waitForSent('publishOffer', (m) => m.tracks.some((t) => t.kind === 'screen'));
	const remoteScreen = guest.tile(`${OWNER_NAME}’s screen`);
	await expect(remoteScreen).toBeVisible({ timeout: 15_000 });

	// Expanded: the screen alone, fullscreen on the meeting, and the control bar still there.
	const header = guest.meeting.locator('header');
	await expect(header).toBeVisible();
	await remoteScreen.getByRole('button', { name: 'Expand screen' }).click();
	await expect(guest.tiles).toHaveCount(1);
	await expect(header).toHaveCount(0);
	await expect(guest.control('Mute')).toBeVisible();
	await expect
		.poll(() => guest.page.evaluate(() => document.fullscreenElement?.className ?? ''))
		.toContain('meeting');
	await guest.control('Mute').click();
	await expect(guest.control('Unmute')).toBeVisible();

	await remoteScreen.getByRole('button', { name: 'Exit expanded view' }).click();
	await expect(guest.tiles).toHaveCount(3);
	await expect(header).toBeVisible();
	await expect.poll(() => guest.page.evaluate(() => document.fullscreenElement)).toBeNull();

	// Leaving fullscreen another way (Esc, a system gesture) also leaves the expanded view.
	await remoteScreen.getByRole('button', { name: 'Expand screen' }).click();
	await expect(guest.tiles).toHaveCount(1);
	await expect
		.poll(() => guest.page.evaluate(() => document.fullscreenElement !== null))
		.toBe(true);
	await guest.page.evaluate(() => document.exitFullscreen());
	await expect(guest.tiles).toHaveCount(3);

	// The share ends while expanded: back to the normal stage.
	await remoteScreen.getByRole('button', { name: 'Expand screen' }).click();
	await expect(guest.tiles).toHaveCount(1);
	await expect
		.poll(() => guest.page.evaluate(() => document.fullscreenElement !== null))
		.toBe(true);
	await host.control('Stop sharing').click();
	await expect(host.tile('Your screen')).toHaveCount(0);
	await expect(remoteScreen).toHaveCount(0);
	await expect(guest.tiles).toHaveCount(2);
	await expect(header).toBeVisible();
	await expect.poll(() => guest.page.evaluate(() => document.fullscreenElement)).toBeNull();
});

test('devices: switching camera and mic in the meeting swaps the tracks without a new publish', async ({
	api,
	owner,
	host,
	guest,
	browserName
}) => {
	test.skip(browserName !== 'chromium', 'only Chromium fakes more than one device per kind');
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await startCall(meeting, host, [{ who: guest, name: 'Guest' }]);
	const hostOnGuest = guest.tile(OWNER_NAME);
	await expect(hostOnGuest.locator('video')).toBeAttached();
	await expect
		.poll(() => sentTrackLabels(host.page))
		.toEqual({
			audio: 'Fake Default Audio Input',
			video: 'fake_device_0'
		});
	const offers = host.sent('publishOffer').length;

	await host.control('Devices').click();
	const dialog = host.page.getByRole('dialog', { name: 'Microphone and camera' });
	await expect(dialog).toBeVisible();
	const camera = dialog.getByRole('combobox', { name: 'Camera' });
	const microphone = dialog.getByRole('combobox', { name: 'Microphone' });
	await camera.selectOption({ label: 'fake_device_1' });
	await microphone.selectOption({ label: 'Fake Audio Input 2' });

	// The new tracks went onto the existing senders: no unpublish, no new offer.
	await expect
		.poll(() => sentTrackLabels(host.page))
		.toEqual({
			audio: 'Fake Audio Input 2',
			video: 'fake_device_1'
		});
	expect(host.sent('unpublish')).toEqual([]);
	expect(host.sent('publishOffer')).toHaveLength(offers);
	await expect(camera.locator('option:checked')).toHaveText('fake_device_1');
	await expect(microphone.locator('option:checked')).toHaveText('Fake Audio Input 2');

	await expect(host.tile(`${OWNER_NAME} (you)`).locator('video')).toBeAttached();
	await expect(hostOnGuest.locator('video')).toBeAttached();
	const hostView = guest
		.received('state')
		.at(-1)
		?.state.participants.find((p) => p.role === 'host');
	expect(hostView?.tracks).toEqual(expect.arrayContaining(['audio', 'video']));

	await dialog.getByRole('button', { name: 'Done' }).click();
	await expect(dialog).toBeHidden();
	await host.control('Mute').click();
	await expect(host.control('Unmute')).toHaveAttribute('aria-pressed', 'true');
});

test('a guest leaves; the host ends the meeting for everyone, with the transcript link', async ({
	api,
	owner,
	host,
	guest,
	people
}) => {
	const meeting = await createMeeting(api, owner.headers, 'fromStart');
	const third = await people.open('third');
	await startCall(meeting, host, [
		{ who: guest, name: 'Guest' },
		{ who: third, name: 'Third' }
	]);

	await guest.control('Leave').click();
	await expect(guest.page.getByRole('heading', { name: 'You left the meeting' })).toBeVisible();
	await expect(host.tiles).toHaveCount(2);

	await host.endForAll();
	for (const p of [host, third]) {
		await expect(p.page.getByRole('heading', { name: 'The meeting has ended' })).toBeVisible();
		await expect(p.transcriptLink).toHaveAttribute('href', /\/t\/[0-9a-f]{32}$/);
	}

	const late = await people.open('late');
	await late.page.goto(`/${meeting}`);
	await expect(late.page.getByRole('heading', { name: /has ended|does not exist/ })).toBeVisible();
});
