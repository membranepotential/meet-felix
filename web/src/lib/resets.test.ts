import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RESET_RETRY_MS, SessionResets } from './resets.ts';

describe('SessionResets', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('asks once per kind until the reset arrives', () => {
		const request = vi.fn();
		const resets = new SessionResets(request);
		expect(resets.failed('subscribe', 0)).toBe(true);
		expect(resets.failed('subscribe', 0)).toBe(false);
		expect(resets.failed('publish', 0)).toBe(true);
		expect(request.mock.calls).toEqual([
			['subscribe', 0],
			['publish', 0]
		]);

		resets.done('subscribe');
		expect(resets.failed('subscribe', 0)).toBe(true);
		expect(request).toHaveBeenCalledTimes(3);
	});

	it('asks again every retry period while no reset arrives', () => {
		const request = vi.fn();
		const resets = new SessionResets(request);
		resets.failed('publish', 0);
		vi.advanceTimersByTime(RESET_RETRY_MS - 1);
		expect(request).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(1);
		expect(request).toHaveBeenCalledTimes(2);
		// The repeat is no new failure: the caller shows no second notice.
		expect(resets.failed('publish', 0)).toBe(false);

		resets.done('publish');
		vi.advanceTimersByTime(10 * RESET_RETRY_MS);
		expect(request).toHaveBeenCalledTimes(2);
	});

	it('stops asking when cancelled', () => {
		const request = vi.fn();
		const resets = new SessionResets(request);
		resets.failed('publish', 0);
		resets.failed('subscribe', 0);
		resets.cancel();
		vi.advanceTimersByTime(10 * RESET_RETRY_MS);
		expect(request).toHaveBeenCalledTimes(2);
	});
});
