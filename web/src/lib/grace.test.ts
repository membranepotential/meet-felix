import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GraceNotice } from './grace.ts';

describe('GraceNotice', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('stays quiet when the problem fixes itself within the grace period', () => {
		const show = vi.fn();
		const notice = new GraceNotice(10_000, show);
		notice.start('lost');
		vi.advanceTimersByTime(3_000);
		notice.cancel();
		vi.advanceTimersByTime(20_000);
		expect(show).not.toHaveBeenCalled();
	});

	it('shows once after the grace period, counted from the first start', () => {
		const show = vi.fn();
		const notice = new GraceNotice(10_000, show);
		notice.start('lost');
		vi.advanceTimersByTime(6_000);
		notice.start('lost');
		vi.advanceTimersByTime(4_000);
		expect(show).toHaveBeenCalledExactlyOnceWith('lost');
	});

	it('starts a new grace period after one ran out', () => {
		const show = vi.fn();
		const notice = new GraceNotice(10_000, show);
		notice.start('lost');
		vi.advanceTimersByTime(10_000);
		notice.start('lost');
		vi.advanceTimersByTime(9_999);
		expect(show).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(1);
		expect(show).toHaveBeenCalledTimes(2);
	});
});
