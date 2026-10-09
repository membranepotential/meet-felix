/**
 * Measures how loud each voice in the meeting is, for the active-speaker ring and level bars.
 * One `AnalyserNode` per audio track, all in one `AudioContext`, read 10 times a second.
 * Nothing is played through the context; remote audio plays through `<audio>` elements.
 */
import { levelFromRms, rms, smoothLevel } from './speaking.ts';

const POLL_MS = 100;
const FFT_SIZE = 512;
/** Levels are kept in steps of this size, so tiny changes do not re-render the tiles. */
const STEP = 0.05;

interface Meter {
	track: MediaStreamTrack;
	source: MediaStreamAudioSourceNode;
	analyser: AnalyserNode;
}

const quantize = (level: number) => Math.round(level / STEP) * STEP;

/** Levels by key; replaced as a whole on change, never mutated. */
export type Levels = Readonly<Record<string, number>>;

const sameLevels = (a: Levels, b: Levels) => {
	const keys = Object.keys(a);
	return keys.length === Object.keys(b).length && keys.every((key) => a[key] === b[key]);
};

export class AudioLevels {
	/** The smoothed level in 0..1 of each measured track by key; silent keys are absent. */
	levels = $state.raw<Levels>({});

	#context: AudioContext | null = null;
	// Web Audio bookkeeping, never read by the UI, so it needs no reactivity.
	// eslint-disable-next-line svelte/prefer-svelte-reactivity
	#meters = new Map<string, Meter>();
	#samples = new Float32Array(FFT_SIZE);
	#timer: ReturnType<typeof setInterval> | null = null;

	/**
	 * Measures exactly `tracks`: starts meters for new tracks and stops those for gone ones.
	 * Writes no reactive state, so an `$effect` may call it; the next poll drops stale levels.
	 */
	sync(tracks: ReadonlyMap<string, MediaStreamTrack>): void {
		for (const [key, meter] of this.#meters) {
			if (tracks.get(key) === meter.track) continue;
			meter.source.disconnect();
			this.#meters.delete(key);
		}
		for (const [key, track] of tracks) {
			if (this.#meters.has(key)) continue;
			const context = this.#audioContext();
			const source = context.createMediaStreamSource(new MediaStream([track]));
			const analyser = context.createAnalyser();
			analyser.fftSize = FFT_SIZE;
			source.connect(analyser);
			this.#meters.set(key, { track, source, analyser });
		}
		this.#timer ??= setInterval(() => this.#poll(), POLL_MS);
	}

	dispose(): void {
		if (this.#timer !== null) clearInterval(this.#timer);
		this.#timer = null;
		for (const { source } of this.#meters.values()) source.disconnect();
		this.#meters.clear();
		void this.#context?.close();
		this.#context = null;
	}

	#audioContext(): AudioContext {
		if (!this.#context) {
			this.#context = new AudioContext();
			// Created after the user pressed Join, so browsers let it run.
			this.#context.resume().catch(() => {});
		}
		return this.#context;
	}

	#poll(): void {
		const next: Record<string, number> = {};
		for (const [key, { analyser }] of this.#meters) {
			analyser.getFloatTimeDomainData(this.#samples);
			const measured = levelFromRms(rms(this.#samples));
			const level = quantize(smoothLevel(this.levels[key] ?? 0, measured));
			if (level > 0) next[key] = level;
		}
		if (!sameLevels(next, this.levels)) this.levels = next;
	}
}
