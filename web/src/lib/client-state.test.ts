import { describe, expect, it } from 'vitest';
import { INITIAL, reduce, type ClientState } from './client-state.ts';
import type { ParticipantView, RoomView } from './protocol.ts';

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

const welcomed = (you = 1, view = room()): ClientState =>
	reduce(INITIAL, { type: 'welcome', you, state: view, resume: 'r1' })[0];

describe('reduce', () => {
	it('enters on welcome, keeps the resume token and asks for fresh media', () => {
		const [state, commands] = reduce(INITIAL, {
			type: 'welcome',
			you: 1,
			state: room(),
			resume: 'r1'
		});
		expect(state.stage).toEqual({ kind: 'in', you: 1, room: room() });
		expect(state.resume).toBe('r1');
		expect(commands).toEqual([{ kind: 'welcomed', fresh: true }]);
	});

	it('keeps media after a resumed welcome but renews it for a new participant id', () => {
		const resumed = reduce(welcomed(1), { type: 'welcome', you: 1, state: room(), resume: 'r1' });
		expect(resumed[1]).toEqual([{ kind: 'welcomed', fresh: false }]);
		const replaced = reduce(welcomed(1), { type: 'welcome', you: 7, state: room(), resume: 'r2' });
		expect(replaced[1]).toEqual([{ kind: 'welcomed', fresh: true }]);
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

	it('passes SDP through to the media layer', () => {
		const tracks = [{ mid: '0', track: { owner: 2, kind: 'video' as const } }];
		expect(reduce(welcomed(), { type: 'subscribeOffer', sdp: 'o', tracks })[1]).toEqual([
			{ kind: 'subscribe-offer', sdp: 'o', tracks }
		]);
		expect(reduce(welcomed(), { type: 'publishAnswer', sdp: 'a' })[1]).toEqual([
			{ kind: 'publish-answer', sdp: 'a' }
		]);
	});
});
