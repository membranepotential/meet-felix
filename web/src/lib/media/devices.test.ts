import { describe, expect, it } from 'vitest';
import { uniqueById } from './devices.ts';

describe('uniqueById', () => {
	it('keeps the first device per id and the order', () => {
		const devices = [
			{ id: 'a', label: 'Webcam' },
			{ id: 'b', label: 'Capture card' },
			{ id: 'a', label: 'Webcam (again)' }
		];
		expect(uniqueById(devices)).toEqual([devices[0], devices[1]]);
	});

	it('leaves a list without duplicates alone', () => {
		const devices = [{ id: 'a', label: 'A' }];
		expect(uniqueById(devices)).toEqual(devices);
	});
});
