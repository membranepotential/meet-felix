/**
 * Camera, microphone and screen capture with constraints tuned for meetings.
 */

/** Selected input devices; `null` means "browser default". */
export interface DeviceChoice {
	microphone: string | null;
	camera: string | null;
}

/** Which inputs to capture. */
export interface CaptureWants {
	audio: boolean;
	video: boolean;
}

export type CaptureError =
	| 'denied' // the user or a policy refused permission
	| 'not-found' // no such device
	| 'in-use' // another app holds the device (common on Windows)
	| 'overconstrained' // the chosen device cannot satisfy the constraints
	| 'insecure' // not a secure context, `mediaDevices` is missing
	| 'unknown';

export type CaptureResult = { ok: true; stream: MediaStream } | { ok: false; error: CaptureError };

/** What to tell the user on the pre-join screen about a capture that failed. */
export function describeCaptureError(error: CaptureError): string {
	switch (error) {
		case 'denied':
			return 'Camera and microphone are blocked. Allow them in your browser’s site settings, then try again.';
		case 'not-found':
			return 'No camera or microphone was found. You can still join and listen.';
		case 'in-use':
			return 'Another app is using your camera or microphone. Close it, then try again.';
		case 'overconstrained':
			return 'The selected device cannot be used. Pick another one.';
		case 'insecure':
			return 'This page needs HTTPS to use your camera and microphone.';
		case 'unknown':
			return 'Camera and microphone could not start. Try again.';
	}
}

/** What to tell the user about a device switch inside the meeting that failed. */
export function describeSwitchError(error: CaptureError): string {
	switch (error) {
		case 'not-found':
			return 'That device was not found.';
		case 'denied':
			return 'The browser blocks this device. Allow it in the site settings.';
		case 'in-use':
			return 'Another app is using that device.';
		case 'overconstrained':
			return 'That device cannot be used. Pick another one.';
		case 'insecure':
		case 'unknown':
			return 'That device could not start. Pick another one.';
	}
}

/**
 * 720p at 30 fps is the top simulcast layer; the SFU forwards lower layers to small tiles.
 * `ideal` instead of `exact` so phones with other native modes still succeed.
 */
const VIDEO: MediaTrackConstraints = {
	width: { ideal: 1280 },
	height: { ideal: 720 },
	frameRate: { ideal: 30, max: 30 },
	facingMode: 'user'
};

const AUDIO: MediaTrackConstraints = {
	echoCancellation: true,
	noiseSuppression: true,
	autoGainControl: true
};

/** Pure: builds `getUserMedia` constraints from the user's choice. */
function userMediaConstraints(choice: DeviceChoice, wants: CaptureWants): MediaStreamConstraints {
	const withDevice = (base: MediaTrackConstraints, id: string | null): MediaTrackConstraints =>
		id === null ? base : { ...base, deviceId: { exact: id } };
	return {
		audio: wants.audio ? withDevice(AUDIO, choice.microphone) : false,
		video: wants.video ? withDevice(VIDEO, choice.camera) : false
	};
}

export async function captureUserMedia(
	choice: DeviceChoice,
	wants: CaptureWants
): Promise<CaptureResult> {
	if (!navigator.mediaDevices?.getUserMedia) return { ok: false, error: 'insecure' };
	if (!wants.audio && !wants.video) return { ok: true, stream: new MediaStream() };
	try {
		const stream = await navigator.mediaDevices.getUserMedia(userMediaConstraints(choice, wants));
		return { ok: true, stream };
	} catch (e) {
		return { ok: false, error: classifyError(e) };
	}
}

function classifyError(e: unknown): CaptureError {
	if (!(e instanceof DOMException)) return 'unknown';
	switch (e.name) {
		case 'NotAllowedError':
		case 'SecurityError':
			return 'denied';
		case 'NotFoundError':
			return 'not-found';
		case 'NotReadableError':
		case 'AbortError':
			return 'in-use';
		case 'OverconstrainedError':
			return 'overconstrained';
		default:
			return 'unknown';
	}
}

/** What the browser reports about itself, read from features and media queries only (D13). */
export interface DeviceTraits {
	/** `navigator.mediaDevices.getDisplayMedia` exists. */
	displayMedia: boolean;
	/** The primary pointer is a finger: `(pointer: coarse)`. */
	coarsePointer: boolean;
	/** Some input device can hover: `(any-hover: hover)`. */
	hover: boolean;
}

function deviceTraits(): DeviceTraits {
	const matches = (query: string) => typeof matchMedia === 'function' && matchMedia(query).matches;
	return {
		displayMedia:
			typeof navigator !== 'undefined' &&
			typeof navigator.mediaDevices?.getDisplayMedia === 'function',
		coarsePointer: matches('(pointer: coarse)'),
		hover: matches('(any-hover: hover)')
	};
}

/** A phone or tablet: touch is the main input and nothing can hover. */
export const isPhoneLike = (traits: DeviceTraits): boolean => traits.coarsePointer && !traits.hover;

/**
 * Screen sharing is for desktops. Phone and tablet browsers lack `getDisplayMedia`; a touch
 * device that claims it anyway (an emulator, a headless browser) is still treated as a phone.
 */
export const canShareScreen = (traits: DeviceTraits = deviceTraits()): boolean =>
	traits.displayMedia && !isPhoneLike(traits);

export type ScreenResult =
	| { ok: true; track: MediaStreamTrack }
	| { ok: false; error: 'cancelled' | 'unsupported' | 'unknown' };

/**
 * Captures a screen, window or tab. The track gets `contentHint = "detail"` so encoders keep
 * text sharp and drop frame rate instead of resolution under load.
 */
export async function captureScreen(): Promise<ScreenResult> {
	if (!canShareScreen()) return { ok: false, error: 'unsupported' };
	try {
		const stream = await navigator.mediaDevices.getDisplayMedia({
			video: { frameRate: { ideal: 15, max: 30 } },
			audio: false
		});
		const [track] = stream.getVideoTracks();
		if (!track) return { ok: false, error: 'unknown' };
		track.contentHint = 'detail';
		return { ok: true, track };
	} catch (e) {
		// The user closed the picker: not an error worth showing.
		if (e instanceof DOMException && e.name === 'NotAllowedError') {
			return { ok: false, error: 'cancelled' };
		}
		return { ok: false, error: 'unknown' };
	}
}
