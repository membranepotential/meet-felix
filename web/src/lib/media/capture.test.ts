import { describe, expect, it } from 'vitest';
import { canShareScreen, isPhoneLike, type DeviceTraits } from './capture.ts';

const desktop: DeviceTraits = { displayMedia: true, coarsePointer: false, hover: true };
const phone: DeviceTraits = { displayMedia: false, coarsePointer: true, hover: false };

describe('isPhoneLike', () => {
	it('treats a touch-only device as a phone', () => {
		expect(isPhoneLike(phone)).toBe(true);
	});

	it('does not treat a touch laptop or a tablet with a trackpad as a phone', () => {
		expect(isPhoneLike({ ...desktop, coarsePointer: true })).toBe(false);
	});
});

describe('canShareScreen', () => {
	it('shows "Share screen" on a desktop with getDisplayMedia', () => {
		expect(canShareScreen(desktop)).toBe(true);
	});

	it('hides it where getDisplayMedia is missing', () => {
		expect(canShareScreen({ ...desktop, displayMedia: false })).toBe(false);
		expect(canShareScreen(phone)).toBe(false);
	});

	it('hides it on a touch-only device that claims getDisplayMedia', () => {
		expect(canShareScreen({ ...phone, displayMedia: true })).toBe(false);
	});
});
