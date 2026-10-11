import { describe, expect, it } from 'vitest';
import {
	awaitingConsent,
	describeError,
	INITIAL,
	over,
	reduce,
	syncMute,
	type ClientState
} from './client-state.ts';
import type { ParticipantView, RoomView, SessionEpochs } from './protocol.ts';

const person = (
	id: number,
	name: string,
	extra: Partial<ParticipantView> = {}
): ParticipantView => ({
	id,
	name,
	role: 'guest',
	consentRecording: true,
	presence: 'joined',
	tracks: [],
	muted: false,
	reconnecting: false,
	...extra
});

const room = (extra: Partial<RoomView> = {}): RoomView => ({
	meeting: 'amber-otter-lamp',
	phase: 'live',
	locked: false,
	recordingPolicy: 'hostDecides',
	participants: [person(1, 'Ada', { role: 'host' })],
	...extra
});

const EPOCHS: SessionEpochs = { publish: 0, subscribe: 0 };

const welcomed = (you = 1, view = room()): ClientState =>
	reduce(INITIAL, { type: 'welcome', you, state: view, resume: 'r1', epochs: EPOCHS })[0];

describe('reduce', () => {
	it('enters on welcome, keeps the resume token and asks for fresh media', () => {
		const [state, commands] = reduce(INITIAL, {
			type: 'welcome',
			you: 1,
			state: room(),
			resume: 'r1',
			epochs: EPOCHS
		});
		expect(state.stage).toEqual({ kind: 'in', you: 1, room: room() });
		expect(state.resume).toBe('r1');
		expect(commands).toEqual([{ kind: 'welcomed', fresh: true, epochs: EPOCHS }]);
	});

	it('keeps media after a resumed welcome but renews it for a new participant id', () => {
		const resumed = reduce(welcomed(1), {
			type: 'welcome',
			you: 1,
			state: room(),
			resume: 'r1',
			epochs: EPOCHS
		});
		expect(resumed[1]).toEqual([{ kind: 'welcomed', fresh: false, epochs: EPOCHS }]);
		const replaced = reduce(welcomed(1), {
			type: 'welcome',
			you: 7,
			state: room(),
			resume: 'r2',
			epochs: EPOCHS
		});
		expect(replaced[1]).toEqual([{ kind: 'welcomed', fresh: true, epochs: EPOCHS }]);
		expect(replaced[0].resume).toBe('r2');
	});

	it('shows the lobby while waiting and opens the consent dialog on request', () => {
		const [waiting] = reduce(INITIAL, { type: 'waiting' });
		expect(waiting.stage.kind).toBe('waiting');
		const [asked] = reduce(waiting, { type: 'consentRequired' });
		expect(asked.consentAsked).toBe(true);
		expect(asked.stage.kind).toBe('waiting');
	});

	it('announces a recording start and stop from the room view', () => {
		const on = room({ recording: { since: 5, by: 1 } });
		const [started, startCommands] = reduce(welcomed(), { type: 'state', state: on });
		expect(startCommands).toEqual([{ kind: 'toast', text: 'Recording started', tone: 'alert' }]);
		const [, stopCommands] = reduce(started, { type: 'state', state: room() });
		expect(stopCommands).toEqual([{ kind: 'toast', text: 'Recording stopped', tone: 'alert' }]);
	});

	it('mutes the microphone on request', () => {
		const [, commands] = reduce(welcomed(), { type: 'muted' });
		expect(commands[0]).toEqual({ kind: 'mute-mic' });
	});

	it('ends the stay on kicked and ended, then ignores further messages', () => {
		const [kicked, commands] = reduce(welcomed(), { type: 'kicked' });
		expect(kicked.stage).toEqual({ kind: 'over', end: { kind: 'kicked' } });
		expect(commands).toEqual([{ kind: 'teardown' }]);
		expect(reduce(kicked, { type: 'state', state: room() })).toEqual([kicked, []]);

		const [ended] = reduce(welcomed(), { type: 'ended', transcript: 'abc' });
		expect(ended.stage).toEqual({ kind: 'over', end: { kind: 'ended', transcript: 'abc' } });
		const [unrecorded] = reduce(welcomed(), { type: 'ended' });
		expect(unrecorded.stage).toEqual({ kind: 'over', end: { kind: 'ended', transcript: null } });
	});

	it('turns errors into notices and recovery commands', () => {
		const publish = reduce(welcomed(), { type: 'error', code: 'publishFailed' })[1];
		expect(publish.map((c) => c.kind)).toEqual(['publish-rejected', 'toast']);
		expect(reduce(welcomed(), { type: 'error', code: 'notSubscribed' })[1]).toEqual([
			{ kind: 'layer-refused' }
		]);
		const unpublish = reduce(welcomed(), { type: 'error', code: 'notPublished' })[1];
		expect(unpublish.map((c) => c.kind)).toEqual(['warn']);
		expect(reduce(welcomed(), { type: 'error', code: 'subscribeFailed' })[1]).toEqual([
			{
				kind: 'subscribe-retrying',
				notice: 'Some video or audio did not arrive. The meeting retries on its own.'
			}
		]);
	});

	it('ends the stay on a full room, with a final notice, and ignores what follows', () => {
		const [state, commands] = reduce(INITIAL, { type: 'error', code: 'roomFull' });
		expect(describeError('roomFull')).toBe('This meeting is full. Try again later.');
		expect(state.stage).toEqual({
			kind: 'over',
			end: { kind: 'refused', reason: 'This meeting is full. Try again later.' }
		});
		expect(commands).toEqual([{ kind: 'teardown' }]);
		expect(reduce(state, { type: 'error', code: 'roomFull' })).toEqual([state, []]);
	});

	it('ends the stay of a page the room calls out of date, and reloads it', () => {
		const [state, commands] = reduce(INITIAL, { type: 'error', code: 'outdatedClient' });
		expect(state.stage).toEqual({
			kind: 'over',
			end: {
				kind: 'refused',
				reason: 'This page is out of date. Reload it to join the meeting.'
			}
		});
		expect(commands).toEqual([{ kind: 'teardown' }, { kind: 'reload' }]);
	});

	it('passes SDP through to the media layer', () => {
		const tracks = [{ mid: '0', track: { owner: 2, kind: 'video' as const } }];
		expect(reduce(welcomed(), { type: 'subscribeOffer', sdp: 'o', tracks })[1]).toEqual([
			{ kind: 'subscribe-offer', sdp: 'o', tracks }
		]);
		expect(reduce(welcomed(), { type: 'publishAnswer', sdp: 'a' })[1]).toEqual([
			{ kind: 'publish-answer', sdp: 'a', rejected: [] }
		]);
		expect(reduce(welcomed(), { type: 'publishAnswer', sdp: 'a', rejected: ['video'] })[1]).toEqual(
			[{ kind: 'publish-answer', sdp: 'a', rejected: ['video'] }]
		);
	});

	it("turns the room's session resets into new connections of that kind", () => {
		expect(reduce(welcomed(), { type: 'subscribeReset', epoch: 1 })).toEqual([
			welcomed(),
			[{ kind: 'subscribe-reset', epoch: 1 }]
		]);
		expect(reduce(welcomed(), { type: 'publishReset', epoch: 2 })).toEqual([
			welcomed(),
			[{ kind: 'publish-reset', epoch: 2 }]
		]);
	});
});

describe('syncMute', () => {
	it('switches the mic off when the room muted us, since muting wins', () => {
		// A host muted us while the socket was down: the room has our last report, "unmuted".
		expect(syncMute(true, true, false)).toBe('mute-mic');
		// Muted when we joined, as the room records without our consent.
		expect(syncMute(true, true, null)).toBe('mute-mic');
	});

	it('reports a mic switched off before joining', () => {
		expect(syncMute(false, false, null)).toBe('report');
	});

	it('reports a switch the room did not hear of while the socket was down', () => {
		expect(syncMute(true, true, true)).toBe('report');
		expect(syncMute(false, false, false)).toBe('report');
	});

	it('does nothing when both agree', () => {
		expect(syncMute(true, false, true)).toBe('none');
		expect(syncMute(false, true, null)).toBe('none');
	});
});

describe('over', () => {
	it('ends the stay, forgets a pending consent question and tears down', () => {
		const asked: ClientState = { ...INITIAL, consentAsked: true };
		expect(over(asked, { kind: 'left' })).toEqual([
			{ ...INITIAL, stage: { kind: 'over', end: { kind: 'left' } } },
			[{ kind: 'teardown' }]
		]);
	});
});

describe('awaitingConsent', () => {
	const recording = { since: 0, by: 1 };
	const participants = [
		person(1, 'Ada', { role: 'host', consentRecording: false }),
		person(2, 'Bo', { consentRecording: false }),
		person(3, 'Cy', { consentRecording: false, presence: 'awaitingConsent' }),
		person(4, 'Di'),
		person(5, 'Ed', { consentRecording: false, presence: 'knocking' }),
		person(6, 'Fay', { consentRecording: false, presence: 'connecting' }),
		// The owner's second device: a host without consent is admitted, muted and unrecorded.
		person(7, 'Gil', { role: 'host', consentRecording: false })
	];

	it('lists the others in the room or let in who have not accepted the running recording', () => {
		const names = awaitingConsent(room({ recording, participants }), 1).map((p) => p.name);
		expect(names).toEqual(['Bo', 'Cy', 'Fay', 'Gil']);
	});

	it('is empty while nothing records', () => {
		expect(awaitingConsent(room({ participants }), 1)).toEqual([]);
	});
});
