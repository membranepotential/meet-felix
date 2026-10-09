/**
 * Local camera and mic tracks as an immutable record, plus replacement helpers.
 * Replacing returns a new record (so Svelte `$state.raw` sees the change) and stops the
 * replaced track, which releases the device. Senders swap tracks without renegotiation.
 */

export type MediaKind = 'audio' | 'video';

export interface LocalTracks {
	audio: MediaStreamTrack | null;
	video: MediaStreamTrack | null;
}

export const NO_TRACKS: LocalTracks = { audio: null, video: null };

export const isMediaKind = (kind: string): kind is MediaKind =>
	kind === 'audio' || kind === 'video';

export function tracksOf(stream: MediaStream): LocalTracks {
	return { audio: stream.getAudioTracks()[0] ?? null, video: stream.getVideoTracks()[0] ?? null };
}

/** Puts `next` in its kind's slot and stops the track it replaces. */
export function withTrack(tracks: LocalTracks, next: MediaStreamTrack): LocalTracks {
	if (!isMediaKind(next.kind)) return tracks;
	const previous = tracks[next.kind];
	if (previous !== next) previous?.stop();
	return { ...tracks, [next.kind]: next };
}

/** Empties one slot and stops its track (camera off releases the camera light). */
export function withoutTrack(tracks: LocalTracks, kind: MediaKind): LocalTracks {
	tracks[kind]?.stop();
	return { ...tracks, [kind]: null };
}

export function stopTracks(tracks: LocalTracks): void {
	tracks.audio?.stop();
	tracks.video?.stop();
}

/** A fresh stream for a `<video>` element; a new object makes attachments rebind. */
export function streamOf(...tracks: (MediaStreamTrack | null)[]): MediaStream | null {
	const present = tracks.filter((t): t is MediaStreamTrack => t !== null);
	return present.length > 0 ? new MediaStream(present) : null;
}

/**
 * Puts `track` on an existing RTP sender. `null` sends nothing while keeping the
 * transceiver, which is how camera-off works without renegotiating with the SFU.
 * Returns `false` if the browser refused (e.g. the transceiver was stopped).
 */
export async function replaceSenderTrack(
	sender: RTCRtpSender,
	track: MediaStreamTrack | null
): Promise<boolean> {
	try {
		await sender.replaceTrack(track);
		return true;
	} catch {
		return false;
	}
}
