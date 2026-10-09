/**
 * Recording state is part of the room's state and announced to everyone (D15): a recorded
 * meeting records from the host's arrival, only with consent, and its end carries the
 * transcript link.
 */
import { createMeeting } from '../fixtures/api.ts';
import { joinAs, liveMeeting } from '../fixtures/meeting.ts';
import { expect, test } from '../fixtures/test.ts';

test('a meeting recorded from the start records from the host’s arrival, and End carries the transcript token', async ({
	api,
	owner,
	rooms
}) => {
	const { host, guest } = await liveMeeting({ api, owner, rooms }, 'fromStart');
	await host.client.take(
		'state',
		(m) =>
			m.state.recording?.by === host.id &&
			m.state.participants.some((p) => p.id === guest.id && p.presence === 'joined')
	);
	host.client.send({ type: 'end' });
	const [forHost, forGuest] = await Promise.all([
		host.client.take('ended'),
		guest.client.take('ended')
	]);
	expect(forHost.transcript).toMatch(/^[0-9a-f]{32}$/);
	expect(forGuest.transcript).toBe(forHost.transcript);
});

test('a guest without consent waits outside a recorded meeting until they consent', async ({
	api,
	owner,
	rooms
}) => {
	const meeting = await createMeeting(api, owner.headers, 'fromStart');
	const host = await joinAs(rooms, meeting, 'Host', owner.headers);
	const guest = await rooms.open(meeting);
	guest.join({ name: 'Guest', consent: false });
	await guest.take('consentRequired');
	await host.client.take('state', (m) =>
		m.state.participants.some((p) => p.name === 'Guest' && p.presence === 'awaitingConsent')
	);
	guest.send({ type: 'setConsent', consent: true });
	const welcome = await guest.take('welcome');
	expect(welcome.state.participants.find((p) => p.id === welcome.you)?.consentRecording).toBe(true);
});

test('in a meeting where the host decides, recording starts and stops with the host, and asks the others', async ({
	api,
	owner,
	rooms
}) => {
	const meeting = await createMeeting(api, owner.headers, 'hostDecides');
	const host = await joinAs(rooms, meeting, 'Host', owner.headers);
	const guest = await rooms.open(meeting);
	guest.join({ name: 'Guest', consent: false });
	const welcome = await guest.take('welcome');
	expect(welcome.state.recording).toBeUndefined();

	host.client.send({ type: 'setRecording', on: true });
	await guest.take('state', (m) => m.state.recording?.by === host.id);
	await guest.take('consentRequired');
	await guest.take('muted');

	host.client.send({ type: 'setRecording', on: false });
	await guest.take('state', (m) => m.state.recording === undefined);
});

test('a guest cannot start the recording', async ({ api, owner, rooms }) => {
	const { guest } = await liveMeeting({ api, owner, rooms }, 'hostDecides');
	guest.client.send({ type: 'setRecording', on: true });
	await guest.client.take('error', (m) => m.code === 'notHost');
});
