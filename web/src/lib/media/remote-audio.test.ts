import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RemoteAudio } from './remote-audio.svelte.ts';

/** Whether the fake browser refuses `play()` as it does without a tap. */
let refusing = false;
let created: FakeAudio[] = [];

class FakeAudio {
	srcObject: unknown = null;
	paused = true;
	autoplay = false;
	plays = 0;

	constructor() {
		created.push(this);
	}

	play(): Promise<void> {
		this.plays++;
		if (refusing) return Promise.reject(new DOMException('needs a tap', 'NotAllowedError'));
		this.paused = false;
		return Promise.resolve();
	}

	pause(): void {
		this.paused = true;
	}

	remove(): void {}
}

class FakeAudioContext {
	state: AudioContextState = 'running';
	resume = () => Promise.resolve();
	close = () => Promise.resolve();
	createMediaStreamDestination = () => ({ stream: { silent: true } });
}

/** Stand-ins for `MediaStream`s; the pool only compares them. */
const stream = (name: string) => ({ name }) as unknown as MediaStream;
/** Lets the `play()` promises settle. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('RemoteAudio', () => {
	beforeEach(() => {
		refusing = false;
		created = [];
		vi.spyOn(console, 'warn').mockImplementation(() => {});
		vi.stubGlobal('AudioContext', FakeAudioContext);
		vi.stubGlobal('document', {
			visibilityState: 'visible',
			createElement: () => new FakeAudio(),
			body: { append() {} },
			addEventListener() {},
			removeEventListener() {}
		});
	});
	afterEach(() => {
		vi.unstubAllGlobals();
		vi.restoreAllMocks();
	});

	it('starts one element in the Join tap and gives it to the first participant', () => {
		const audio = new RemoteAudio();
		expect(created).toHaveLength(1);
		expect(created[0]?.plays).toBe(1);
		const voice = stream('ada');
		audio.sync(new Map([[1, voice]]));
		expect(created).toHaveLength(1);
		expect(created[0]?.srcObject).toBe(voice);
	});

	it('keeps a participant’s element through a session reset and swaps only its stream', () => {
		const audio = new RemoteAudio();
		const [before, after] = [stream('before reset'), stream('after reset')];
		audio.sync(new Map([[1, before]]));
		audio.sync(new Map([[1, null]]));
		audio.sync(new Map([[1, after]]));
		expect(created).toHaveLength(1);
		expect(created[0]?.srcObject).toBe(after);
		expect(created[0]?.plays).toBe(3);
	});

	it('gives the element of someone who left to the next one who joins', () => {
		const audio = new RemoteAudio();
		audio.sync(
			new Map([
				[1, stream('ada')],
				[2, stream('marina')]
			])
		);
		expect(created).toHaveLength(2);
		audio.sync(new Map([[2, stream('marina')]]));
		audio.sync(
			new Map([
				[2, stream('marina')],
				[3, stream('grace')]
			])
		);
		expect(created).toHaveLength(2);
	});

	it('asks for a tap while the browser refuses to play a voice, and stops once it plays', async () => {
		const audio = new RemoteAudio();
		refusing = true;
		audio.sync(new Map([[1, stream('ada')]]));
		await settle();
		expect(audio.blocked).toBe(true);
		refusing = false;
		audio.resume();
		await settle();
		expect(audio.blocked).toBe(false);
	});

	it('does not ask for a tap when nobody is there to hear', async () => {
		refusing = true;
		const audio = new RemoteAudio();
		await settle();
		expect(audio.blocked).toBe(false);
		audio.sync(new Map([[1, null]]));
		await settle();
		expect(audio.blocked).toBe(false);
	});
});
