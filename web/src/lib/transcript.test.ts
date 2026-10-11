import { describe, expect, it } from 'vitest';
import { duration, minutes, timeline, type Transcript } from './transcript.ts';

/** A `transcript.json` as the recorder writes it. */
const transcript: Transcript = {
	version: 1,
	meeting: 'amber-otter-ocean',
	startedAt: 1_000,
	endedAt: 3_601_000,
	speakers: [
		{ who: 0, label: 'Robin (host)', role: 'host' },
		{ who: 3, label: 'Anna', role: 'guest' }
	],
	consents: [
		{ who: 0, name: 'Robin', at: 800, noticeVersion: 1 },
		{ who: 3, name: 'Anna', at: 900, noticeVersion: 1, withdrawnAt: 50_000 }
	],
	gaps: [{ who: 3, start: 10_000, end: 22_000, reason: 'missing' }],
	paragraphs: [
		{ who: 0, label: 'Robin (host)', startMs: 1_000, endMs: 4_000, text: 'Guten Tag.' },
		{ who: 3, label: 'Anna', startMs: 10_000, endMs: 12_000, text: 'Hello.' }
	]
};

describe('transcript page helpers', () => {
	it('lists gaps between paragraphs in time order, labelled with the speaker', () => {
		expect(timeline(transcript).map((e) => e.kind)).toEqual(['paragraph', 'gap', 'paragraph']);
		const gap = timeline(transcript).find((e) => e.kind === 'gap');
		expect(gap?.kind === 'gap' ? gap.label : null).toBe('Anna');
	});

	it('words durations and rounds minutes up', () => {
		expect([duration(12_000), duration(12 * 60_000), duration(65 * 60_000)]).toEqual([
			'12 s',
			'12 min',
			'1 h 5 min'
		]);
		expect([minutes(1), minutes(60_000), minutes(60_001)]).toEqual([1, 1, 2]);
	});
});
