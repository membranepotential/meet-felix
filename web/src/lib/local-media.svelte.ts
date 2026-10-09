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
import { guardTracks, type TrackGuard } from './media/resilience.ts';
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

export class LocalMedia {
	tracks = $state.raw<LocalTracks>(NO_TRACKS);
	screen = $state.raw<MediaStreamTrack | null>(null);
	devices = $state.raw<DeviceLists>({ microphones: [], cameras: [] });
	choice = $state<DeviceChoice>({ microphone: null, camera: null });
	/** The user's mute switch. Muting keeps the track live (and published) with `enabled = false`. */
	micOn = $state(true);
	error = $state.raw<CaptureError | null>(null);
	starting = $state(false);

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

	constructor() {
		this.#guard = guardTracks({
			reacquire: (kind) => this.#capture(kind),
			onReplaced: (next) => this.#install(next)
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
		const track = await this.#capture('video');
		if (track) this.#install(track);
	}

	async selectMicrophone(id: string): Promise<void> {
		this.choice.microphone = id;
		const track = await this.#capture('audio');
		if (track) this.#install(track);
	}

	async selectCamera(id: string): Promise<void> {
		this.choice.camera = id;
		// Switching devices while the camera is off only records the choice.
		if (!this.tracks.video) return;
		// iOS opens one camera at a time: release the old one before asking for the new one.
		this.#guard.unwatch(this.tracks.video);
		this.tracks = withoutTrack(this.tracks, 'video');
		const track = await this.#capture('video');
		if (track) this.#install(track);
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
		this.#guard.dispose();
		this.#unwatchDevices();
		stopTracks(this.tracks);
		this.tracks = NO_TRACKS;
		this.stopScreenShare();
	}

	async #capture(kind: MediaKind): Promise<MediaStreamTrack | null> {
		const wants: CaptureWants = { audio: kind === 'audio', video: kind === 'video' };
		const result = await captureUserMedia(this.choice, wants);
		if (!result.ok) {
			this.error = result.error;
			return null;
		}
		this.error = null;
		return tracksOf(result.stream)[kind];
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
