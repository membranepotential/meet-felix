/** Browser flows that several specs share: a meeting with the host and guests in it. */
import { expect } from '@playwright/test';
import type { MeetingId } from '../../src/lib/protocol.ts';
import type { Participant } from './participant.ts';

export { OWNER_NAME } from './stack.ts';

/** The host opens the link: the pre-join knows the owner and prefills the name. */
export async function hostPreJoin(host: Participant, meeting: MeetingId): Promise<void> {
	await host.openPreJoin(meeting);
	await expect(host.page.getByText('You join as host')).toBeVisible();
}

/** The host joins first, then each guest; everyone ends up in the meeting with all tiles. */
export async function startCall(
	meeting: MeetingId,
	host: Participant,
	guests: readonly { who: Participant; name: string }[]
): Promise<void> {
	await hostPreJoin(host, meeting);
	await host.join();
	await expect(host.meeting).toBeVisible();
	for (const { who, name } of guests) {
		await who.openPreJoin(meeting, name);
		await who.join();
		await expect(who.meeting).toBeVisible();
	}
	const everyone = [host, ...guests.map((g) => g.who)];
	for (const p of everyone) await expect(p.tiles).toHaveCount(everyone.length);
}
