/**
 * Simulcast layer choice per received camera track (docs/media/README.md, D11): the tile's
 * rendered width picks the layer, and changes go to the room as `ClientMsg::SetLayer`,
 * debounced so a window resize sends one message per track, not one per frame.
 */
import { trackKey, type Rid, type TrackRef } from './protocol.ts';

/** Tiles narrower than this (CSS pixels) get the quarter-resolution layer. */
export const QUARTER_BELOW_PX = 240;
/** Tiles narrower than this (CSS pixels) get the half-resolution layer; wider ones get full. */
export const HALF_BELOW_PX = 640;
export const LAYER_DEBOUNCE_MS = 400;

/** The layer a new subscription starts at (D11). */
export const DEFAULT_LAYER: Rid = 'f';

/** Pure: the layer for a tile `width` CSS pixels wide. */
export const layerFor = (width: number): Rid =>
	width < QUARTER_BELOW_PX ? 'q' : width < HALF_BELOW_PX ? 'h' : DEFAULT_LAYER;

/** What the room was last asked for, per track key; absent means `DEFAULT_LAYER`. */
export type SentLayers = ReadonlyMap<string, Rid>;

/**
 * Pure: the `SetLayer` requests that bring each received track to the layer its tile needs.
 * `widths` holds the last measured width per track key; only `received` tracks qualify,
 * because the room rejects `SetLayer` for a track it has not delivered yet.
 */
export function layerChanges(
	widths: ReadonlyMap<string, { track: TrackRef; width: number }>,
	received: ReadonlySet<string>,
	sent: SentLayers
): { track: TrackRef; rid: Rid }[] {
	return [...widths]
		.filter(([key]) => received.has(key))
		.map(([key, { track, width }]) => ({ key, track, rid: layerFor(width) }))
		.filter(({ key, rid }) => (sent.get(key) ?? DEFAULT_LAYER) !== rid)
		.map(({ track, rid }) => ({ track, rid }));
}

/** Collects tile widths and sends the resulting layer changes after a quiet period. */
export class LayerControl {
	readonly #send: (track: TrackRef, rid: Rid) => void;
	readonly #widths = new Map<string, { track: TrackRef; width: number }>();
	readonly #received = new Set<string>();
	readonly #sent = new Map<string, Rid>();
	#timer: ReturnType<typeof setTimeout> | undefined;

	constructor(send: (track: TrackRef, rid: Rid) => void) {
		this.#send = send;
	}

	/** A tile showing `track` is now `width` CSS pixels wide. */
	report(track: TrackRef, width: number): void {
		this.#widths.set(trackKey(track), { track, width });
		this.#schedule();
	}

	/** A tile for `track` is gone; its width no longer counts. */
	forget(track: TrackRef): void {
		this.#widths.delete(trackKey(track));
	}

	/** A subscribe negotiation delivered `tracks`: they start at the SFU default again. */
	received(tracks: readonly TrackRef[]): void {
		for (const track of tracks) {
			const key = trackKey(track);
			this.#received.add(key);
			this.#sent.delete(key);
		}
		this.#schedule();
	}

	dispose(): void {
		clearTimeout(this.#timer);
	}

	#schedule(): void {
		clearTimeout(this.#timer);
		this.#timer = setTimeout(() => this.#flush(), LAYER_DEBOUNCE_MS);
	}

	#flush(): void {
		for (const { track, rid } of layerChanges(this.#widths, this.#received, this.#sent)) {
			this.#sent.set(trackKey(track), rid);
			this.#send(track, rid);
		}
	}
}
