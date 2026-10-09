import { describe, expect, it } from 'vitest';
import { layerChanges, layerFor } from './layers.ts';
import type { TrackRef } from './protocol.ts';

describe('layerFor', () => {
	it('picks low below 240 px, mid below 640 px, high otherwise', () => {
		expect([0, 239, 240, 639, 640, 1920].map(layerFor)).toEqual([
			'low',
			'low',
			'mid',
			'mid',
			'high',
			'high'
		]);
	});
});

describe('layerChanges', () => {
	const a: TrackRef = { owner: 1, kind: 'video' };
	const b: TrackRef = { owner: 2, kind: 'video' };
	const widths = new Map([
		['1:video', { track: a, width: 200 }],
		['2:video', { track: b, width: 900 }]
	]);

	it('asks only for received tracks whose layer differs from the last request', () => {
		const received = new Set(['1:video', '2:video']);
		// b stays at the SFU default (high), so only a changes.
		expect(layerChanges(widths, received, new Map())).toEqual([{ track: a, rid: 'low' }]);
		expect(layerChanges(widths, received, new Map([['1:video', 'low']]))).toEqual([]);
	});

	it('skips tracks the room has not delivered yet', () => {
		expect(layerChanges(widths, new Set(['2:video']), new Map())).toEqual([]);
	});
});
