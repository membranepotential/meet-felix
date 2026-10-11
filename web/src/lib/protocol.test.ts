import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it } from 'vitest';
import { initSync } from './wasm/meeting_core.js';
import {
	encodeClientMsg,
	meetingWords,
	parseMeetingId,
	parseServerMsg,
	trackKey,
	type ClientMsg,
	type RoomView,
	type ServerMsg
} from './protocol.ts';

beforeAll(() => {
	initSync({ module: readFileSync(new URL('./wasm/meeting_core_bg.wasm', import.meta.url)) });
});

/** One sample per variant; the mapped type fails the build when the core adds a variant. */
type Samples<M extends { type: string }> = { [T in M['type']]: Extract<M, { type: T }> };

const room: RoomView = {
	meeting: 'abide-acid-acorn',
	phase: 'live',
	locked: false,
	recordingPolicy: 'hostDecides',
	recording: { since: 1_700_000_000_000, by: 1 },
	participants: [
		{
			id: 1,
			name: 'Ada',
			role: 'host',
			consentRecording: true,
			presence: 'joined',
			tracks: ['audio', 'video'],
			muted: true,
			reconnecting: true
		}
	]
};

const SERVER_SAMPLES: Samples<ServerMsg> = {
	welcome: {
		type: 'welcome',
		you: 1,
		state: room,
		resume: 'r1',
		epochs: { publish: 0, subscribe: 2 }
	},
	state: { type: 'state', state: room },
	waiting: { type: 'waiting' },
	publishAnswer: { type: 'publishAnswer', sdp: 'v=0' },
	subscribeOffer: {
		type: 'subscribeOffer',
		sdp: 'v=0',
		tracks: [{ mid: '0', track: { owner: 2, kind: 'screen' } }]
	},
	publishReset: { type: 'publishReset', epoch: 1 },
	subscribeReset: { type: 'subscribeReset', epoch: 3 },
	knock: { type: 'knock', who: 3 },
	consentRequired: { type: 'consentRequired' },
	muted: { type: 'muted' },
	kicked: { type: 'kicked' },
	ended: { type: 'ended', transcript: 'abc' },
	error: { type: 'error', code: 'notPublished' }
};

const CLIENT_SAMPLES: Samples<ClientMsg> = {
	join: { type: 'join', name: 'Ada', consent: true, resume: 'r1', protocol: 2 },
	setConsent: { type: 'setConsent', consent: false },
	setMuted: { type: 'setMuted', muted: true },
	publishOffer: {
		type: 'publishOffer',
		sdp: 'v=0',
		tracks: [{ mid: '0', kind: 'video', name: 'cam-1' }]
	},
	unpublish: { type: 'unpublish', kind: 'video' },
	subscribeAnswer: { type: 'subscribeAnswer', sdp: 'v=0' },
	setLayer: { type: 'setLayer', track: { owner: 2, kind: 'video' }, rid: 'h' },
	resetSession: { type: 'resetSession', kind: 'subscribe', epoch: 3 },
	lock: { type: 'lock', on: true },
	admit: { type: 'admit', who: 3 },
	kick: { type: 'kick', who: 3 },
	mute: { type: 'mute', who: 3 },
	setRecording: { type: 'setRecording', on: true },
	end: { type: 'end' },
	leave: { type: 'leave' }
};

describe('meetingWords', () => {
	it('splits an id into its words', () => {
		expect(meetingWords('abide-acid-acorn')).toEqual(['abide', 'acid', 'acorn']);
	});
});

describe('parseMeetingId', () => {
	it('accepts listed words and lowercases what phones capitalise', () => {
		expect(parseMeetingId('abide-acid-acorn')).toBe('abide-acid-acorn');
		expect(parseMeetingId('Abide-Acid-acorn')).toBe('abide-acid-acorn');
	});

	it('rejects ids that are not three listed words', () => {
		for (const bad of [
			'',
			'abide-acid',
			'abide-acid-zzz',
			'abide--acid',
			'abide-acid-acorn-acre'
		]) {
			expect(parseMeetingId(bad)).toBeNull();
		}
	});
});

describe('parseServerMsg', () => {
	it.each(Object.values(SERVER_SAMPLES))('accepts $type', (msg) => {
		expect(parseServerMsg(JSON.stringify(msg))).toEqual(msg);
	});

	it('rejects unknown tags, missing fields, wrong values and non-JSON', () => {
		for (const bad of [
			'{"type":"Welcome"}',
			'{"type":"toString"}',
			'{"type":"pong"}',
			'{"type":"publishAnswer"}',
			'{"type":"error","code":"teapot"}',
			'not json',
			'[]'
		]) {
			expect(() => parseServerMsg(bad), bad).toThrow();
		}
	});
});

describe('encodeClientMsg', () => {
	it.each(Object.values(CLIENT_SAMPLES))('round-trips $type', (msg) => {
		expect(JSON.parse(encodeClientMsg(msg))).toEqual(msg);
	});

	it('rejects a message the room would refuse', () => {
		expect(() => encodeClientMsg({ type: 'join', name: '', consent: true })).toThrow();
	});
});

describe('trackKey', () => {
	it('is distinct per owner and kind', () => {
		expect(trackKey({ owner: 3, kind: 'screen' })).toBe('3:screen');
	});
});
