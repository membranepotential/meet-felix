import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MUTE_GRACE_MS, watchStall } from './resilience.ts';

/** A track whose state the test sets and whose events it fires, like the browser would. */
class FakeTrack extends EventTarget {
	muted = false;
	readyState: MediaStreamTrackState = 'live';

	set(event: 'mute' | 'unmute' | 'ended'): void {
		if (event === 'ended') this.readyState = 'ended';
		else this.muted = event === 'mute';
		this.dispatchEvent(new Event(event));
	}
}

function watch(track: FakeTrack) {
	const changes: boolean[] = [];
	const stop = watchStall(track, (stalled) => changes.push(stalled));
	return { changes, stop };
}

describe('watchStall', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('reports a track muted for the whole grace period, and its unmute', () => {
		const track = new FakeTrack();
		const { changes } = watch(track);
		track.set('mute');
		vi.advanceTimersByTime(MUTE_GRACE_MS - 1);
		expect(changes).toEqual([]);
		vi.advanceTimersByTime(1);
		expect(changes).toEqual([true]);
		track.set('unmute');
		expect(changes).toEqual([true, false]);
	});

	it('ignores a mute shorter than the grace period', () => {
		const track = new FakeTrack();
		const { changes } = watch(track);
		track.set('mute');
		vi.advanceTimersByTime(MUTE_GRACE_MS / 2);
		track.set('unmute');
		vi.advanceTimersByTime(MUTE_GRACE_MS);
		expect(changes).toEqual([]);
	});

	it('reports an ended track at once', () => {
		const track = new FakeTrack();
		const { changes } = watch(track);
		track.set('ended');
		expect(changes).toEqual([true]);
	});

	// iOS after a phone call: the mic stays muted, and no event ever follows.
	it('counts a track that is already muted from the start of the watch', () => {
		const track = new FakeTrack();
		track.muted = true;
		const { changes } = watch(track);
		expect(changes).toEqual([]);
		vi.advanceTimersByTime(MUTE_GRACE_MS);
		expect(changes).toEqual([true]);
	});

	it('reports nothing after the cleanup', () => {
		const track = new FakeTrack();
		const { changes, stop } = watch(track);
		track.set('mute');
		stop();
		vi.advanceTimersByTime(MUTE_GRACE_MS);
		track.set('ended');
		expect(changes).toEqual([]);
	});
});
