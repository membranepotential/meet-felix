/**
 * When the page turns the camera and microphone on, and that it turns them off again: a dead
 * link must not capture anything, and a capture that finishes after leaving must be stopped.
 * The page's `getUserMedia` is wrapped to count calls, to optionally hold the answer back (the
 * permission prompt), and to keep every stream it handed out.
 */
import type { MeetingId } from '../../src/lib/protocol.ts';
import { createMeeting } from '../fixtures/api.ts';
import { hostPreJoin } from '../fixtures/call.ts';
import type { Participant } from '../fixtures/participant.ts';
import { expect, test } from '../fixtures/test.ts';

declare global {
	interface Window {
		__e2eCaptureCalls?: number;
		__e2eCaptured?: MediaStream[];
		/** Answers the `getUserMedia` calls that were held back. */
		__e2eAnswerCaptures?: () => void;
	}
}

/** A listed meeting id that no test creates. */
const UNKNOWN_MEETING = 'abide-acid-acorn' as MeetingId;

/** Runs in the page before its scripts. */
function trackCapture({ hold }: { hold: boolean }): void {
	let answer = () => {};
	const answered = hold ? new Promise<void>((resolve) => (answer = resolve)) : Promise.resolve();
	window.__e2eCaptureCalls = 0;
	window.__e2eCaptured = [];
	window.__e2eAnswerCaptures = answer;
	const devices = navigator.mediaDevices;
	const original = devices.getUserMedia.bind(devices);
	devices.getUserMedia = async (constraints) => {
		window.__e2eCaptureCalls = (window.__e2eCaptureCalls ?? 0) + 1;
		await answered;
		const stream = await original(constraints);
		window.__e2eCaptured?.push(stream);
		return stream;
	};
}

async function watchCapture(who: Participant, options: { hold: boolean }): Promise<void> {
	await who.context.addInitScript(trackCapture, options);
}

const captureCalls = (who: Participant) => who.page.evaluate(() => window.__e2eCaptureCalls ?? 0);

const liveTracks = (who: Participant) =>
	who.page.evaluate(
		() =>
			(window.__e2eCaptured ?? [])
				.flatMap((stream) => stream.getTracks())
				.filter((track) => track.readyState === 'live').length
	);

test('an unknown meeting link captures nothing', async ({ guest }) => {
	await watchCapture(guest, { hold: false });
	await guest.page.goto(`/${UNKNOWN_MEETING}`);
	await expect(
		guest.page.getByRole('heading', { name: 'This meeting does not exist' })
	).toBeVisible();
	expect(await captureCalls(guest)).toBe(0);
	expect(await liveTracks(guest)).toBe(0);
});

test('the link of a meeting that has ended captures nothing', async ({
	api,
	owner,
	host,
	guest
}) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await hostPreJoin(host, meeting);
	await host.join();
	await expect(host.meeting).toBeVisible();
	await host.endForAll();
	await expect(host.page.getByRole('heading', { name: 'The meeting has ended' })).toBeVisible();

	await watchCapture(guest, { hold: false });
	await guest.page.goto(`/${meeting}`);
	// An unrecorded meeting leaves no trace once it ends (D6): its link is unknown again.
	await expect(
		guest.page.getByRole('heading', { name: 'This meeting does not exist' })
	).toBeVisible();
	expect(await captureCalls(guest)).toBe(0);
	expect(await liveTracks(guest)).toBe(0);
});

test('the link of an open meeting captures once the meeting is known', async ({
	api,
	owner,
	guest
}) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await watchCapture(guest, { hold: false });
	await guest.openPreJoin(meeting, 'Guest');
	await expect.poll(() => liveTracks(guest)).toBeGreaterThan(0);
});

test('a capture that finishes after leaving is stopped', async ({ api, owner, host }) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	await watchCapture(host, { hold: true });
	await hostPreJoin(host, meeting);
	await host.join();
	await expect(host.meeting).toBeVisible();
	await host.control('Leave').click();
	await expect(host.page.getByRole('heading', { name: 'You left the meeting' })).toBeVisible();

	// The permission prompt is answered only now.
	expect(await captureCalls(host)).toBeGreaterThan(0);
	await host.page.evaluate(() => window.__e2eAnswerCaptures?.());
	await expect
		.poll(() => host.page.evaluate(() => window.__e2eCaptured?.length ?? 0))
		.toBeGreaterThan(0);
	expect(await liveTracks(host)).toBe(0);
});
