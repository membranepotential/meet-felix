import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
	LAYER_DEBOUNCE_MS,
	LAYER_RETRY_MS,
	LayerControl,
	layerChanges,
	layerFor
} from './layers.ts';
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

	it('asks again for a layer that may have been refused', () => {
		const received = new Set(['1:video', '2:video']);
		expect(layerChanges(widths, received, new Map([['2:video', null]]))).toEqual([
			{ track: a, rid: 'q' },
			{ track: b, rid: 'f' }
		]);
	});
});

describe('LayerControl', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	const track: TrackRef = { owner: 1, kind: 'video' };

	function setup() {
		const sent: Rid[] = [];
		const control = new LayerControl((_, rid) => sent.push(rid));
		control.report(track, 300);
		control.received([track]);
		vi.advanceTimersByTime(LAYER_DEBOUNCE_MS);
		return { control, sent };
	}

	it('sends a refused layer again after the retry delay', () => {
		const { control, sent } = setup();
		expect(sent).toEqual(['h']);
		control.notSubscribed();
		vi.advanceTimersByTime(LAYER_RETRY_MS - 1);
		expect(sent).toEqual(['h']);
		vi.advanceTimersByTime(1);
		expect(sent).toEqual(['h', 'h']);
		// Accepted this time: nothing more goes out.
		vi.advanceTimersByTime(10 * LAYER_RETRY_MS);
		expect(sent).toEqual(['h', 'h']);
	});

	it('sends a refused layer again when the track is delivered', () => {
		const { control, sent } = setup();
		control.notSubscribed();
		control.received([track]);
		vi.advanceTimersByTime(LAYER_DEBOUNCE_MS);
		expect(sent).toEqual(['h', 'h']);
	});

	it('restores the full layer after a refusal', () => {
		const { control, sent } = setup();
		control.notSubscribed();
		control.report(track, 900);
		vi.advanceTimersByTime(LAYER_DEBOUNCE_MS);
		expect(sent).toEqual(['h', 'f']);
	});

	it('after a reset, asks for nothing until the track arrives again, then asks anew', () => {
		const { control, sent } = setup();
		control.reset();
		control.report(track, 300);
		vi.advanceTimersByTime(LAYER_DEBOUNCE_MS);
		expect(sent).toEqual(['h']);
		control.received([track]);
		vi.advanceTimersByTime(LAYER_DEBOUNCE_MS);
		expect(sent).toEqual(['h', 'h']);
	});
});
