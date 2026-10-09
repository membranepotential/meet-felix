/**
 * Keeps local capture alive on mobile, iOS in particular. Safari ends or mutes camera and
 * mic tracks when the page goes to the background, a call comes in, or another app takes
 * the camera, and it does not restart them on its own. The guard notices and reacquires.
 *
 * Signals:
 * - `ended` on a track: the device is gone for good; reacquire at once. (Our own
 *   `track.stop()` does not fire `ended`, so turning the camera off is not mistaken for this.)
 * - `mute` lasting longer than a grace period: the OS interrupted capture.
 * - `visibilitychange` to visible: recheck every track, since iOS may have ended them while
 *   no events were delivered.
 *
 * Where reacquiring cannot work without a tap, `isStalled` tells the UI to offer one: after a
 * phone call, iOS leaves the mic `muted` and fires neither `unmute` nor `ended`, and may not
 * fire `visibilitychange` either (WebKit bugs 213853, 204681, 240651; research 11).
 */

import { createSubscriber } from 'svelte/reactivity';
import { isMediaKind, type MediaKind } from './tracks.ts';

/** How long a track may stay muted before we treat it as lost. */
export const MUTE_GRACE_MS = 2000;

export interface TrackGuardOptions {
	/** Captures a fresh track of `kind`, or `null` if that fails. */
	reacquire: (kind: MediaKind) => Promise<MediaStreamTrack | null>;
	/** Called with the new track; the caller swaps it into its stream and RTP sender. */
	onReplaced: (next: MediaStreamTrack, previous: MediaStreamTrack) => void;
	/** How long a track may stay muted before we treat it as lost; `MUTE_GRACE_MS` by default. */
	muteGraceMs?: number;
}

export interface TrackGuard {
	watch(track: MediaStreamTrack): void;
	unwatch(track: MediaStreamTrack): void;
	dispose(): void;
}

export function guardTracks({
	reacquire,
	onReplaced,
	muteGraceMs = MUTE_GRACE_MS
}: TrackGuardOptions): TrackGuard {
	/** Per watched track: its listener cleanup. */
	const watched = new Map<MediaStreamTrack, () => void>();
	/** One reacquisition per kind at a time. */
	const inFlight = new Set<MediaKind>();
	let disposed = false;

	async function recover(track: MediaStreamTrack): Promise<void> {
		const kind = track.kind;
		if (disposed || !isMediaKind(kind) || inFlight.has(kind) || !watched.has(track)) return;
		inFlight.add(kind);
		try {
			const next = await reacquire(kind);
			if (disposed || next === null || !watched.has(track)) {
				next?.stop();
				return;
			}
			unwatch(track);
			watch(next);
			onReplaced(next, track);
		} finally {
			inFlight.delete(kind);
		}
	}

	function watch(track: MediaStreamTrack): void {
		if (disposed || watched.has(track)) return;
		let muteTimer: ReturnType<typeof setTimeout> | undefined;
		const clearMuteTimer = () => clearTimeout(muteTimer);
		const onEnded = () => void recover(track);
		const onMute = () => {
			clearMuteTimer();
			muteTimer = setTimeout(() => void recover(track), muteGraceMs);
		};
		track.addEventListener('ended', onEnded);
		track.addEventListener('mute', onMute);
		track.addEventListener('unmute', clearMuteTimer);
		watched.set(track, () => {
			clearMuteTimer();
			track.removeEventListener('ended', onEnded);
			track.removeEventListener('mute', onMute);
			track.removeEventListener('unmute', clearMuteTimer);
		});
	}

	function unwatch(track: MediaStreamTrack): void {
		watched.get(track)?.();
		watched.delete(track);
	}

	const onVisibility = () => {
		if (document.visibilityState !== 'visible') return;
		for (const track of [...watched.keys()]) {
			if (track.readyState === 'ended' || track.muted) void recover(track);
		}
	};
	document.addEventListener('visibilitychange', onVisibility);

	return {
		watch,
		unwatch,
		dispose() {
			disposed = true;
			document.removeEventListener('visibilitychange', onVisibility);
			for (const track of [...watched.keys()]) unwatch(track);
		}
	};
}

/** The parts of a `MediaStreamTrack` that tell whether it still delivers sound or pictures. */
export type StallSource = EventTarget & {
	readonly muted: boolean;
	readonly readyState: MediaStreamTrackState;
};

/**
 * Follows whether `track` is stalled: it ended, or it stayed muted for `MUTE_GRACE_MS`. Starts
 * from "stalled" only if the track has already ended, and calls `onChange` with each change
 * after that, never during this call. Returns the cleanup.
 */
export function watchStall(track: StallSource, onChange: (stalled: boolean) => void): () => void {
	let stalled = track.readyState === 'ended';
	let graceTimer: ReturnType<typeof setTimeout> | undefined;
	const set = (next: boolean) => {
		if (next === stalled) return;
		stalled = next;
		onChange(next);
	};
	const onMute = () => {
		clearTimeout(graceTimer);
		graceTimer = setTimeout(() => set(track.muted || track.readyState === 'ended'), MUTE_GRACE_MS);
	};
	const onUnmute = () => {
		clearTimeout(graceTimer);
		set(track.readyState === 'ended');
	};
	const onEnded = () => {
		clearTimeout(graceTimer);
		set(true);
	};
	track.addEventListener('mute', onMute);
	track.addEventListener('unmute', onUnmute);
	track.addEventListener('ended', onEnded);
	// A track can arrive muted, for example one captured while a phone call holds the mic.
	if (track.muted) onMute();
	return () => {
		clearTimeout(graceTimer);
		track.removeEventListener('mute', onMute);
		track.removeEventListener('unmute', onUnmute);
		track.removeEventListener('ended', onEnded);
	};
}

const stallReaders = new WeakMap<MediaStreamTrack, () => boolean>();

/** Whether `track` is stalled, as `watchStall` defines it. Reactive in Svelte. */
export function isStalled(track: MediaStreamTrack): boolean {
	let read = stallReaders.get(track);
	if (!read) {
		let stalled = false;
		const subscribe = createSubscriber((update) => {
			stalled = track.readyState === 'ended';
			return watchStall(track, (next) => {
				stalled = next;
				update();
			});
		});
		read = () => {
			subscribe();
			return stalled;
		};
		stallReaders.set(track, read);
	}
	return read();
}
