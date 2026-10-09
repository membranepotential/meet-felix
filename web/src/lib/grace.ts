/**
 * A notice that waits for a problem to fix itself: `start()` begins a grace period, and the
 * notice shows only if `cancel()` does not come before it runs out. Starting again while a
 * period runs keeps the first deadline, so a problem that keeps coming back still shows.
 */
export class GraceNotice {
	readonly #graceMs: number;
	readonly #show: (text: string) => void;
	#timer: ReturnType<typeof setTimeout> | null = null;

	constructor(graceMs: number, show: (text: string) => void) {
		this.#graceMs = graceMs;
		this.#show = show;
	}

	start(text: string): void {
		if (this.#timer !== null) return;
		this.#timer = setTimeout(() => {
			this.#timer = null;
			this.#show(text);
		}, this.#graceMs);
	}

	cancel(): void {
		if (this.#timer === null) return;
		clearTimeout(this.#timer);
		this.#timer = null;
	}
}
