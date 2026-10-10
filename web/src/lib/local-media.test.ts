import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LocalMedia } from './local-media.svelte.ts';
import type { MediaKind } from './media/tracks.ts';

/** The parts of a `MediaStreamTrack` that `LocalMedia` and its track guard use. */
class FakeTrack extends EventTarget {
	enabled = true;
	muted = false;
	readyState: MediaStreamTrackState = 'live';

	constructor(
		readonly kind: MediaKind,
		readonly deviceId: string
	) {
		super();
	}

	get stopped(): boolean {
		return this.readyState === 'ended';
	}

	stop(): void {
		this.readyState = 'ended';
	}

	getSettings(): MediaTrackSettings {
		return { deviceId: this.deviceId };
	}
}

/** One pending `getUserMedia` call, answered by the test. */
interface Request {
	/** Resolves the call with one fresh track per requested kind and returns them. */
	grant(): FakeTrack[];
	deny(name: string): void;
}

let requests: Request[] = [];

/** The browser's default devices: `mic-0` and `cam-0`. */
const DEFAULT_DEVICE: Record<MediaKind, string> = { audio: 'mic-0', video: 'cam-0' };

/** The device a constraint asks for with `deviceId: { exact }`, else the default one. */
function deviceOf(
	kind: MediaKind,
	constraint: boolean | MediaTrackConstraints | undefined
): string {
	const id = typeof constraint === 'object' ? constraint.deviceId : undefined;
	return typeof id === 'object' && !Array.isArray(id) && typeof id.exact === 'string'
		? id.exact
		: DEFAULT_DEVICE[kind];
}

function getUserMedia(constraints: MediaStreamConstraints): Promise<MediaStream> {
	return new Promise((resolve, reject) => {
		requests.push({
			grant() {
				const kinds = (['audio', 'video'] as const).filter((kind) => constraints[kind]);
				const tracks = kinds.map((kind) => new FakeTrack(kind, deviceOf(kind, constraints[kind])));
				const ofKind = (kind: MediaKind) => tracks.filter((t) => t.kind === kind);
				resolve({
					getAudioTracks: () => ofKind('audio'),
					getVideoTracks: () => ofKind('video')
				} as unknown as MediaStream);
				return tracks;
			},
			deny: (name) => reject(new DOMException('refused', name))
		});
	});
}

/** The oldest unanswered `getUserMedia` call. */
function next(): Request {
	const request = requests.shift();
	if (!request) throw new Error('no getUserMedia call is pending');
	return request;
}

/** Lets awaited promises settle. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A `LocalMedia` holding mic `mic-0` and camera `cam-0`. */
async function started(): Promise<{ media: LocalMedia; mic: FakeTrack; cam: FakeTrack }> {
	const media = new LocalMedia();
	const starting = media.start();
	const [mic, cam] = next().grant();
	await starting;
	if (!mic || !cam) throw new Error('start captured no tracks');
	return { media, mic, cam };
}

describe('LocalMedia device switches', () => {
	beforeEach(() => {
		requests = [];
		vi.stubGlobal('navigator', {
			mediaDevices: {
				getUserMedia,
				enumerateDevices: () => Promise.resolve([]),
				addEventListener() {},
				removeEventListener() {}
			}
		});
		vi.stubGlobal('document', {
			visibilityState: 'visible',
			addEventListener() {},
			removeEventListener() {}
		});
	});
	afterEach(() => {
		vi.unstubAllGlobals();
	});

	it('keeps the last of two quick camera picks and stops the other track', async () => {
		const { media, cam } = await started();
		const first = media.selectCamera('cam-1');
		const second = media.selectCamera('cam-2');
		const [cam1] = next().grant();
		const [cam2] = next().grant();
		await Promise.all([first, second]);
		expect(cam.stopped).toBe(true);
		expect(cam1?.stopped).toBe(true);
		expect(media.tracks.video).toBe(cam2);
		expect(cam2?.stopped).toBe(false);
	});

	it('lets only the last pick set the error, and turns the camera off when it fails', async () => {
		const { media } = await started();
		const first = media.selectCamera('cam-1');
		const second = media.selectCamera('cam-2');
		const [earlier, later] = [next(), next()];
		later.deny('NotReadableError');
		await second;
		const [cam1] = earlier.grant();
		await first;
		expect(cam1?.stopped).toBe(true);
		expect(media.error).toBe('in-use');
		expect(media.tracks.video).toBeNull();
		// The select shows the camera that was on before, and Start video asks for it again.
		expect(media.choice.camera).toBe('cam-0');
	});

	it('ignores a failed camera pick that a later pick overtook', async () => {
		const { media } = await started();
		const first = media.selectCamera('cam-1');
		const second = media.selectCamera('cam-2');
		const [earlier, later] = [next(), next()];
		const [cam2] = later.grant();
		await second;
		earlier.deny('NotFoundError');
		await first;
		expect(media.error).toBeNull();
		expect(media.tracks.video).toBe(cam2);
	});

	it('drops a new camera when the camera is turned off during the switch', async () => {
		const { media } = await started();
		const switching = media.selectCamera('cam-1');
		await media.toggleCamera();
		const [cam1] = next().grant();
		await switching;
		expect(cam1?.stopped).toBe(true);
		expect(media.tracks.video).toBeNull();
	});

	it('keeps the last of two quick mic picks, and the old mic if a pick fails', async () => {
		const { media, mic } = await started();
		const first = media.selectMicrophone('mic-1');
		const second = media.selectMicrophone('mic-2');
		const [earlier, later] = [next(), next()];
		const [mic2] = later.grant();
		await second;
		const [mic1] = earlier.grant();
		await first;
		expect(mic.stopped).toBe(true);
		expect(mic1?.stopped).toBe(true);
		expect(media.tracks.audio).toBe(mic2);

		const failing = media.selectMicrophone('mic-3');
		next().deny('NotFoundError');
		await failing;
		expect(media.error).toBe('not-found');
		expect(media.tracks.audio).toBe(mic2);
		expect(media.choice.microphone).toBe('mic-2');
	});

	it('stops a mic that arrives after dispose and leaves the error alone', async () => {
		const { media } = await started();
		const switching = media.selectMicrophone('mic-1');
		media.dispose();
		const [mic1] = next().grant();
		await switching;
		expect(mic1?.stopped).toBe(true);
		expect(media.tracks.audio).toBeNull();
		expect(media.error).toBeNull();
	});

	it('restarts the same mic and keeps it muted', async () => {
		const { media, mic } = await started();
		media.mute();
		const restarting = media.restartMic();
		const [fresh] = next().grant();
		expect(await restarting).toBe(true);
		await settle();
		expect(mic.stopped).toBe(true);
		expect(media.tracks.audio).toBe(fresh);
		expect(fresh?.deviceId).toBe('mic-0');
		expect(fresh?.enabled).toBe(false);
	});
});
