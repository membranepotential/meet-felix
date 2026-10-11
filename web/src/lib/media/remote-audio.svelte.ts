/**
 * Plays the others' voices, one `<audio>` element per participant.
 *
 * Phone browsers start sound only after a tap on the page (research 11): Chrome on Android
 * needs a user gesture, and WebKit lifts its rule per element once that element played inside
 * one. A session reset (D33) brings new tracks without a tap. So:
 * - the instance is created inside the Join tap, where it resumes the `AudioContext` and starts
 *   one element on silence, which the first participant gets;
 * - each participant keeps their element for the whole stay, and a new stream only replaces
 *   its `srcObject`;
 * - when the browser still refuses, `blocked` turns true, and a tap that calls `resume()`
 *   starts every element again.
 */
import type { ParticipantId } from '../protocol.ts';

/** The rejection of `play()` that means "not without a tap". */
const isAutoplayRefusal = (error: unknown): boolean =>
	error instanceof DOMException && error.name === 'NotAllowedError';

/** Each participant's voice, or `null` while none is received (also during a reset). */
export type Voices = ReadonlyMap<ParticipantId, MediaStream | null>;

export class RemoteAudio {
	/** The context for the level meters (D30). Voices do not play through it. */
	readonly context: AudioContext;
	#blocked = $state(false);

	// Element bookkeeping, never read by the UI, so it needs no reactivity; `#blocked` follows
	// `#refused`. So `sync` reads no reactive state, and an `$effect` may call it.
	// eslint-disable-next-line svelte/prefer-svelte-reactivity
	readonly #sinks = new Map<ParticipantId, HTMLAudioElement>();
	/** Elements whose `play()` the browser refused and that have not played since. */
	// eslint-disable-next-line svelte/prefer-svelte-reactivity
	readonly #refused = new Set<HTMLAudioElement>();
	/** Elements nobody uses: those of people who left, and the one started in the Join tap. */
	readonly #idle: HTMLAudioElement[] = [];

	/** Some voice is held back until the user taps the page. */
	get blocked(): boolean {
		return this.#blocked;
	}

	/** Call inside a tap, such as the Join button's: only there may sound start. */
	constructor() {
		this.context = new AudioContext();
		this.#resumeContext();
		const primed = this.#create();
		primed.srcObject = this.context.createMediaStreamDestination().stream;
		this.#idle.push(primed);
		this.#play(primed);
		document.addEventListener('visibilitychange', this.#onVisibility);
	}

	/**
	 * Plays exactly `voices`: keeps each participant's element, swaps its stream when it changes,
	 * and frees the elements of people who left for the next ones who join.
	 */
	sync(voices: Voices): void {
		for (const [id, audio] of this.#sinks) {
			if (voices.has(id)) continue;
			this.#sinks.delete(id);
			audio.pause();
			audio.srcObject = null;
			this.#unrefuse(audio);
			this.#idle.push(audio);
		}
		for (const [id, stream] of voices) {
			let audio = this.#sinks.get(id);
			if (!audio) {
				audio = this.#idle.pop() ?? this.#create();
				this.#sinks.set(id, audio);
			}
			if (audio.srcObject === stream) continue;
			audio.srcObject = stream;
			if (stream) this.#play(audio);
			else this.#unrefuse(audio);
		}
	}

	/** Call inside a tap: starts every voice the browser held back, and the level meters. */
	resume(): void {
		this.#resumeContext();
		for (const audio of this.#sinks.values()) {
			if (audio.srcObject) this.#play(audio);
		}
	}

	dispose(): void {
		document.removeEventListener('visibilitychange', this.#onVisibility);
		for (const audio of [...this.#sinks.values(), ...this.#idle]) {
			audio.pause();
			audio.srcObject = null;
			audio.remove();
		}
		this.#sinks.clear();
		this.#idle.length = 0;
		this.#refused.clear();
		this.#blocked = false;
		this.context.close().catch(() => {});
	}

	#create(): HTMLAudioElement {
		const audio = document.createElement('audio');
		audio.autoplay = true;
		document.body.append(audio);
		return audio;
	}

	/** Starts `audio`; a refusal while it plays someone's voice sets `blocked`. */
	#play(audio: HTMLAudioElement): void {
		audio.play().then(
			() => this.#unrefuse(audio),
			(error: unknown) => {
				// A new stream interrupted this start; the call for that stream reports for itself.
				if (error instanceof DOMException && error.name === 'AbortError') return;
				console.warn('Remote audio did not start', error);
				const inUse = [...this.#sinks.values()].includes(audio);
				if (inUse && isAutoplayRefusal(error)) {
					this.#refused.add(audio);
					this.#blocked = true;
				}
			}
		);
	}

	#unrefuse(audio: HTMLAudioElement): void {
		this.#refused.delete(audio);
		this.#blocked = this.#refused.size > 0;
	}

	#resumeContext(): void {
		// iOS also reports `interrupted`, for example during a phone call.
		if (this.context.state === 'running' || this.context.state === 'closed') return;
		this.context.resume().catch((error: unknown) => {
			console.warn('The level meters could not start', error);
		});
	}

	/**
	 * Phones pause sound and suspend the context while the screen is locked; try to start both
	 * again on return. If the browser wants a tap for that, `blocked` asks for one.
	 */
	readonly #onVisibility = () => {
		if (document.visibilityState !== 'visible') return;
		this.#resumeContext();
		for (const audio of this.#sinks.values()) {
			if (audio.srcObject && audio.paused) this.#play(audio);
		}
	};
}
