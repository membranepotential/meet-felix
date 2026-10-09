/**
 * Reactive views of remote tracks. Whether a participant publishes a kind comes from the room
 * view (`ParticipantView.tracks`), not from the track: Chromium does not raise `mute` on a
 * received track when its publisher stops sending, so a stopped camera would keep a frozen
 * frame. A received track that ends (its transceiver stopped) stops showing at once.
 */
import { createSubscriber } from 'svelte/reactivity';

const readers = new WeakMap<MediaStreamTrack, () => boolean>();

/** Whether `track` has not ended. Reactive: re-reads on `ended`. */
export function isLive(track: MediaStreamTrack): boolean {
	let read = readers.get(track);
	if (!read) {
		const subscribe = createSubscriber((update) => {
			track.addEventListener('ended', update);
			return () => track.removeEventListener('ended', update);
		});
		read = () => {
			subscribe();
			return track.readyState === 'live';
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
