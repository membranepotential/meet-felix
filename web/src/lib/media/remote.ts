/**
 * Reactive views of remote tracks. When its publisher turns the camera off or stops sharing,
 * a received track stops delivering frames (the browser reports `mute`) or ends, until the
 * room renegotiates the subscription. Tiles show video only while the track is live.
 */
import { createSubscriber } from 'svelte/reactivity';

const readers = new WeakMap<MediaStreamTrack, () => boolean>();

/** Whether `track` delivers media now. Reactive: re-reads on `mute`, `unmute` and `ended`. */
export function isLive(track: MediaStreamTrack): boolean {
	let read = readers.get(track);
	if (!read) {
		const subscribe = createSubscriber((update) => {
			const events = ['mute', 'unmute', 'ended'] as const;
			for (const e of events) track.addEventListener(e, update);
			return () => {
				for (const e of events) track.removeEventListener(e, update);
			};
		});
		read = () => {
			subscribe();
			return track.readyState === 'live' && !track.muted;
		};
		readers.set(track, read);
	}
	return read();
}

const streams = new WeakMap<MediaStreamTrack, MediaStream>();

/**
 * One stream per remote track, so a re-rendered tile keeps its `srcObject` instead of
 * rebinding (which flashes black).
 */
export function streamFor(track: MediaStreamTrack): MediaStream {
	let stream = streams.get(track);
	if (!stream) {
		stream = new MediaStream([track]);
		streams.set(track, stream);
	}
	return stream;
}
