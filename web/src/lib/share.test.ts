import { describe, expect, it } from 'vitest';
import { canShareLink } from './share.ts';

describe('canShareLink', () => {
	it('offers "Share…" where navigator.share exists', () => {
		expect(canShareLink({ share: () => Promise.resolve() })).toBe(true);
	});

	it('hides it where navigator.share is missing', () => {
		expect(canShareLink({})).toBe(false);
		expect(canShareLink({ share: undefined })).toBe(false);
		expect(canShareLink(undefined)).toBe(false);
	});
});
