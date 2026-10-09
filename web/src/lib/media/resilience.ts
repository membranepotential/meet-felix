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
 */

import { isMediaKind, type MediaKind } from './tracks.ts';

export interface TrackGuardOptions {
	/** Captures a fresh track of `kind`, or `null` if that fails. */
	reacquire: (kind: MediaKind) => Promise<MediaStreamTrack | null>;
	/** Called with the new track; the caller swaps it into its stream and RTP sender. */
	onReplaced: (next: MediaStreamTrack, previous: MediaStreamTrack) => void;
	/** How long a track may stay muted before we treat it as lost. */
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
	muteGraceMs = 2000
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
