import { describe, expect, it } from 'vitest';
import { orderVideoCodecs, planPublish, trackName } from './sfu.ts';
import type { TrackKind } from './protocol.ts';

describe('orderVideoCodecs', () => {
	it('puts H.264 first, then VP8, keeps helpers and drops VP9 and AV1', () => {
		const codecs = [
			'video/AV1',
			'video/VP9',
			'video/VP8',
			'video/rtx',
			'video/H264',
			'video/red'
		].map((mimeType) => ({ mimeType }));
		expect(orderVideoCodecs(codecs).map((c) => c.mimeType)).toEqual([
			'video/H264',
			'video/VP8',
			'video/rtx',
			'video/red'
		]);
	});
});

describe('planPublish', () => {
	const none = { audio: null, video: null, screen: null };
	const published = (entries: [TrackKind, string][]) => new Map(entries);

	it('adds new kinds, replaces swapped tracks and stops kinds turned off', () => {
		const plan = planPublish(
			published([
				['audio', 'mic-a'],
				['video', 'cam-a']
			]),
			{ audio: 'mic-b', video: null, screen: 'screen-a' },
			new Map()
		);
		expect(plan).toEqual({ stop: ['video'], replace: ['audio'], add: ['screen'] });
	});

	it('does nothing when the published tracks match', () => {
		expect(
			planPublish(published([['audio', 'mic']]), { ...none, audio: 'mic' }, new Map())
		).toEqual({ stop: [], replace: [], add: [] });
	});

	it('does not retry a failed track, but tries a new one of the same kind', () => {
		const failed = new Map<TrackKind, string>([['video', 'cam-a']]);
		expect(planPublish(new Map(), { ...none, video: 'cam-a' }, failed).add).toEqual([]);
		expect(planPublish(new Map(), { ...none, video: 'cam-b' }, failed).add).toEqual(['video']);
	});
});

describe('trackName', () => {
	it('names tracks by kind and generation', () => {
		expect([trackName('audio', 1), trackName('video', 2), trackName('screen', 3)]).toEqual([
			'mic-1',
			'cam-2',
			'screen-3'
		]);
	});
});
