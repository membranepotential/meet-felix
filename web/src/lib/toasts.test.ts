import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MAX_TOASTS, Toasts } from './toasts.svelte.ts';

const texts = (toasts: Toasts) => toasts.items.map((t) => t.text);

describe('Toasts', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('keeps info toasts for 5 s and alerts for 8 s', () => {
		const toasts = new Toasts();
		toasts.show('joined');
		toasts.show('failed', 'alert');
		vi.advanceTimersByTime(5_000);
		expect(texts(toasts)).toEqual(['failed']);
		vi.advanceTimersByTime(3_000);
		expect(texts(toasts)).toEqual([]);
	});

	it('drops the oldest toast when a new one would exceed the limit', () => {
		const toasts = new Toasts();
		const shown = Array.from({ length: MAX_TOASTS + 1 }, (_, i) => `toast ${i}`);
		for (const text of shown) toasts.show(text);
		expect(texts(toasts)).toEqual(shown.slice(1));
	});

	it('dismisses one toast and leaves the others', () => {
		const toasts = new Toasts();
		toasts.show('a');
		toasts.show('b');
		const [first] = toasts.items;
		if (first) toasts.dismiss(first.id);
		expect(texts(toasts)).toEqual(['b']);
	});
});
