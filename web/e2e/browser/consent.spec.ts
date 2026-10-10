/**
 * Recording consent through the UI against the mock SFU (D15): a host who starts recording in a
 * `hostDecides` meeting sees who has not accepted it yet, in the room or waiting outside.
 */
import type { Participant } from '../fixtures/participant.ts';
import { createMeeting } from '../fixtures/api.ts';
import { OWNER_NAME, startCall } from '../fixtures/call.ts';
import { expect, test } from '../fixtures/test.ts';

/** The page's consent dialog. */
const consentDialog = (p: Participant) =>
	p.page.getByRole('dialog', { name: 'This meeting is being recorded' });

/** The host's notice naming who has not accepted the recording. */
const unconsentedNotice = (p: Participant) =>
	p.page.locator('.stack .pill', { hasText: 'not accepted the recording yet' });

test('the host sees who has not accepted the recording until they do', async ({
	api,
	owner,
	host,
	guest,
	people
}) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await startCall(meeting, host, [{ who: guest, name: 'Guest' }]);
	await expect(unconsentedNotice(host)).toHaveCount(0);

	// Nobody accepted recording on the pre-join, so starting it asks both, the host included.
	await (await host.hostControl('Record')).click();
	await expect(consentDialog(guest)).toBeVisible();
	await consentDialog(host).getByRole('button', { name: 'Accept' }).click();
	await expect(unconsentedNotice(host)).toHaveText('Guest has not accepted the recording yet');
	await expect(host.tile('Guest').locator('.unrecorded')).toHaveText('Not recorded');
	await expect(host.tile(`${OWNER_NAME} (you)`).locator('.unrecorded')).toHaveCount(0);
	// Guests do not see it.
	await expect(unconsentedNotice(guest)).toHaveCount(0);
	await expect(guest.page.locator('.unrecorded')).toHaveCount(0);

	// A guest arriving without consent waits outside, and the host sees them too.
	const late = await people.open('late');
	await late.openPreJoin(meeting, 'Late');
	await late.join();
	await expect(consentDialog(late)).toBeVisible();
	await expect(unconsentedNotice(host)).toHaveText(
		'Guest and Late have not accepted the recording yet'
	);

	await consentDialog(guest).getByRole('button', { name: 'Accept' }).click();
	await expect(unconsentedNotice(host)).toHaveText('Late has not accepted the recording yet');
	await expect(host.tile('Guest').locator('.unrecorded')).toHaveCount(0);

	await consentDialog(late).getByRole('button', { name: 'Accept' }).click();
	await expect(late.meeting).toBeVisible();
	await expect(unconsentedNotice(host)).toHaveCount(0);
	await expect(host.tiles).toHaveCount(3);
});
