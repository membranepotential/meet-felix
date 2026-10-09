import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it } from 'vitest';
import { initSync } from './wasm/meeting_core.js';
import { parseMeetingId, parseServerMsg, trackKey } from './protocol.ts';

beforeAll(() => {
	initSync({ module: readFileSync(new URL('./wasm/meeting_core_bg.wasm', import.meta.url)) });
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
	it('accepts known tags and rejects the rest', () => {
		expect(parseServerMsg('{"type":"waiting"}')).toEqual({ type: 'waiting' });
		expect(parseServerMsg('{"type":"Welcome"}')).toBeNull();
		expect(parseServerMsg('{"type":"toString"}')).toBeNull();
		expect(parseServerMsg('not json')).toBeNull();
		expect(parseServerMsg('[]')).toBeNull();
	});
});

describe('trackKey', () => {
	it('is distinct per owner and kind', () => {
		expect(trackKey({ owner: 3, kind: 'screen' })).toBe('3:screen');
	});
});
