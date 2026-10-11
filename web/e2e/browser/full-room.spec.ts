/**
 * A full room (20 present participants, MAX_PRESENT in the core): a guest who presses Join gets
 * a final "meeting is full" screen, and the page stops talking to the room instead of retrying.
 */
import { createMeeting } from '../fixtures/api.ts';
import { joinAs } from '../fixtures/meeting.ts';
import { expect, test } from '../fixtures/test.ts';

/** `MAX_PRESENT` in `crates/meeting-core/src/state.rs`. */
const ROOM_SIZE = 20;

test('a guest who finds the room full sees a final notice and the page does not retry', async ({
	api,
	owner,
	rooms,
	guest
}) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await joinAs(rooms, meeting, 'Host', owner.headers);
	for (let i = 1; i < ROOM_SIZE; i++) await joinAs(rooms, meeting, `Guest ${i}`);

	await guest.openPreJoin(meeting, 'Late');
	await guest.join();

	await expect(
		guest.page.getByRole('heading', { name: 'You are not in the meeting' })
	).toBeVisible();
	await expect(guest.page.getByText('This meeting is full. Try again later.')).toBeVisible();
	// The first retry of a lost connection would come within a second.
	await guest.page.waitForTimeout(2_000);
	expect(guest.sockets).toBe(1);
});
