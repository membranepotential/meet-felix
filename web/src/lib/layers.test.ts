import { describe, expect, it } from 'vitest';
import { layerChanges, layerFor } from './layers.ts';
import type { Rid, TrackRef } from './protocol.ts';

describe('layerFor', () => {
	it('picks quarter below 240 px, half below 640 px, full otherwise', () => {
		const expected: Rid[] = ['q', 'q', 'h', 'h', 'f', 'f'];
		expect([0, 239, 240, 639, 640, 1920].map(layerFor)).toEqual(expected);
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
		// b stays at the default (full), so only a changes.
		expect(layerChanges(widths, received, new Map())).toEqual([{ track: a, rid: 'q' }]);
		expect(layerChanges(widths, received, new Map<string, Rid>([['1:video', 'q']]))).toEqual([]);
		// A track asked down to quarter goes back to full when its tile grows.
		expect(layerChanges(widths, received, new Map<string, Rid>([['2:video', 'q']]))).toEqual([
			{ track: a, rid: 'q' },
			{ track: b, rid: 'f' }
		]);
	});

	it('skips tracks the room has not delivered yet', () => {
		expect(layerChanges(widths, new Set(['2:video']), new Map())).toEqual([]);
	});
});
