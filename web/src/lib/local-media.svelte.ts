/**
 * The local participant's camera, mic and screen share, shared by the pre-join screen and
 * the meeting (tracks acquired for the preview are the ones that get published).
 */
import {
	captureScreen,
	captureUserMedia,
	type CaptureError,
	type CaptureWants,
	type DeviceChoice
} from './media/capture.ts';
import { listDevices, watchDevices, type DeviceLists } from './media/devices.ts';
import { guardTracks, isStalled, type TrackGuard } from './media/resilience.ts';
import type { Outgoing } from './sfu.ts';
import {
	NO_TRACKS,
	stopTracks,
	streamOf,
	tracksOf,
	withTrack,
	withoutTrack,
	type LocalTracks,
	type MediaKind
} from './media/tracks.ts';

/** One captured track, or why capturing it failed. */
type TrackResult = { ok: true; track: MediaStreamTrack } | { ok: false; error: CaptureError };

/**
 * How a capture that replaces a track ended: the new track is in its slot, the capture failed
 * (`error` says why), or a later decision overtook it and the new track was stopped.
 */
type Replacement = 'installed' | 'failed' | 'overtaken';

export class LocalMedia {
	tracks = $state.raw<LocalTracks>(NO_TRACKS);
	screen = $state.raw<MediaStreamTrack | null>(null);
	devices = $state.raw<DeviceLists>({ microphones: [], cameras: [] });
	choice = $state<DeviceChoice>({ microphone: null, camera: null });
	/**
	 * The user's mute switch. Muting keeps the track live (and published) with `enabled = false`;
	 * the session reports the switch to the room, which shows it to everyone.
	 */
	micOn = $state(true);
	error = $state.raw<CaptureError | null>(null);
	starting = $state(false);

	/**
	 * The mic is on but has delivered no sound for `MUTE_GRACE_MS`: the system muted it or it
	 * ended. On iOS this lasts after a phone call until `restartMic()` runs inside a tap.
	 */
	micStalled = $derived(this.micOn && this.tracks.audio !== null && isStalled(this.tracks.audio));
	/** Camera on = a live video track exists; turning it off releases the camera. */
	camOn = $derived(this.tracks.video !== null);
	cameraStream = $derived(streamOf(this.tracks.video));
	screenStream = $derived(streamOf(this.screen));
	/** What to publish: the session hands this to the SFU publisher whenever it changes. */
	outgoing: Outgoing = $derived({
		audio: this.tracks.audio,
		video: this.tracks.video,
		screen: this.screen
	});

	#guard: TrackGuard;
	#unwatchDevices: () => void;
	/** `dispose()` ran: captures that finish later are stopped, not installed. */
	#disposed = false;

	constructor() {
		this.#guard = guardTracks({
			reacquire: async (kind) => {
				const before = this.tracks[kind];
				const result = await this.#capture(kind);
				if (result.ok) return result.track;
				// The guard drops the result if the track was replaced meanwhile; so does the error.
				if (!this.#disposed && this.tracks[kind] === before) this.error = result.error;
				return null;
			},
			onReplaced: (next) => {
				this.error = null;
				this.#install(next);
			}
		});
		this.#unwatchDevices = watchDevices(() => void this.refreshDevices());
	}

	async start(): Promise<void> {
		this.starting = true;
		let result = await captureUserMedia(this.choice, { audio: true, video: true });
		// Desktops without a webcam are common: fall back to the mic alone.
		if (!result.ok && result.error === 'not-found') {
			result = await captureUserMedia(this.choice, { audio: true, video: false });
		}
		this.starting = false;
		if (!result.ok) {
			this.error = result.error;
			await this.refreshDevices();
			return;
		}
		this.error = null;
		this.#forEachTrack((t) => this.#guard.unwatch(t));
		stopTracks(this.tracks);
		this.tracks = tracksOf(result.stream);
		this.#applyMic();
		this.#forEachTrack((t) => this.#guard.watch(t));
		// Labels are only visible once a permission is granted.
		await this.refreshDevices();
	}

	async refreshDevices(): Promise<void> {
		this.devices = await listDevices();
	}

	toggleMic(): void {
		this.micOn = !this.micOn;
		this.#applyMic();
	}

	/** A host asked, or the room records without our consent. */
	mute(): void {
		this.micOn = false;
		this.#applyMic();
	}

	async toggleCamera(): Promise<void> {
		if (this.tracks.video) {
			this.#guard.unwatch(this.tracks.video);
			this.tracks = withoutTrack(this.tracks, 'video');
			return;
		}
		await this.#replace('video');
	}

	/** The last pick wins; if its capture fails, the old mic stays and `choice` points to it again. */
	async selectMicrophone(id: string): Promise<void> {
		const before = this.tracks.audio?.getSettings().deviceId ?? this.choice.microphone;
		this.choice.microphone = id;
		const outcome = await this.#replace('audio', () => this.choice.microphone === id);
		if (outcome === 'failed') this.choice.microphone = before;
	}

	/**
	 * Captures the chosen mic again and puts the new track in place of the old one, also on the
	 * publisher's sender. Call it inside a tap: WebKit captures only after a user action.
	 * Returns `false` if capturing failed (`error` says why); a capture that a later one overtook
	 * does not count as failed.
	 */
	async restartMic(): Promise<boolean> {
		return (await this.#replace('audio')) !== 'failed';
	}

	/**
	 * The last pick wins. If its capture fails, the camera is off and `choice` points to the
	 * camera that was on before.
	 */
	async selectCamera(id: string): Promise<void> {
		const previous = this.tracks.video;
		const before = previous?.getSettings().deviceId ?? this.choice.camera;
		this.choice.camera = id;
		// Switching devices while the camera is off only records the choice.
		if (!previous) return;
		// iOS opens one camera at a time: release the old one before asking for the new one. It
		// stays in its slot meanwhile, so the camera counts as on and the publisher swaps the new
		// track onto its sender instead of publishing the camera anew.
		this.#guard.unwatch(previous);
		previous.stop();
		const outcome = await this.#replace('video', () => this.choice.camera === id);
		if (outcome === 'failed') {
			this.tracks = withoutTrack(this.tracks, 'video');
			this.choice.camera = before;
		}
	}

	/** Returns `false` if the user cancelled the picker or sharing failed. */
	async startScreenShare(): Promise<boolean> {
		const result = await captureScreen();
		if (!result.ok) return false;
		this.screen?.stop();
		this.screen = result.track;
		// The browser's own "Stop sharing" button ends the track.
		result.track.addEventListener('ended', () => {
			if (this.screen === result.track) this.screen = null;
		});
		return true;
	}

	stopScreenShare(): void {
		this.screen?.stop();
		this.screen = null;
	}

	dispose(): void {
		this.#disposed = true;
		this.#guard.dispose();
		this.#unwatchDevices();
		stopTracks(this.tracks);
		this.tracks = NO_TRACKS;
		this.stopScreenShare();
	}

	/**
	 * Captures a fresh `kind` track from the chosen device and puts it in its slot, unless a
	 * later decision overtook the capture: `dispose()` ran, the slot no longer holds the track it
	 * held at the start, or `stillWanted` says no. An overtaken track is stopped, and only a
	 * capture that was not overtaken sets or clears `error`.
	 */
	async #replace(kind: MediaKind, stillWanted: () => boolean = () => true): Promise<Replacement> {
		const before = this.tracks[kind];
		const result = await this.#capture(kind);
		if (this.#disposed || this.tracks[kind] !== before || !stillWanted()) {
			if (result.ok) result.track.stop();
			return 'overtaken';
		}
		if (!result.ok) {
			this.error = result.error;
			return 'failed';
		}
		this.error = null;
		this.#install(result.track);
		return 'installed';
	}

	/** Captures a fresh `kind` track from the chosen device; no side effects on this object. */
	async #capture(kind: MediaKind): Promise<TrackResult> {
		const wants: CaptureWants = { audio: kind === 'audio', video: kind === 'video' };
		const result = await captureUserMedia(this.choice, wants);
		if (!result.ok) return result;
		const track = tracksOf(result.stream)[kind];
		return track ? { ok: true, track } : { ok: false, error: 'unknown' };
	}

	/**
	 * Puts a fresh track in place of the old one of its kind. The new `outgoing` record makes
	 * the publisher swap it onto the RTP sender.
	 */
	#install(track: MediaStreamTrack): void {
		const previous = track.kind === 'audio' ? this.tracks.audio : this.tracks.video;
		if (previous) this.#guard.unwatch(previous);
		this.tracks = withTrack(this.tracks, track);
		this.#guard.watch(track);
		if (track.kind === 'audio') this.#applyMic();
	}

	#applyMic(): void {
		if (this.tracks.audio) this.tracks.audio.enabled = this.micOn;
	}

	#forEachTrack(action: (track: MediaStreamTrack) => void): void {
		if (this.tracks.audio) action(this.tracks.audio);
		if (this.tracks.video) action(this.tracks.video);
	}
}
