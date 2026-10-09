/** Pure geometry for the participant grid. */

export interface GridFit {
	cols: number;
	rows: number;
	tileWidth: number;
	tileHeight: number;
}

/**
 * Chooses the column count that gives `count` tiles of the given aspect ratio the largest
 * size inside `width` × `height`, with `gap` pixels between tiles.
 */
export function fitGrid(
	count: number,
	width: number,
	height: number,
	gap: number,
	aspect: number
): GridFit {
	const n = Math.max(1, count);
	let best: GridFit = { cols: 1, rows: n, tileWidth: 0, tileHeight: 0 };
	for (let cols = 1; cols <= n; cols++) {
		const rows = Math.ceil(n / cols);
		const maxWidth = (width - gap * (cols - 1)) / cols;
		const maxHeight = (height - gap * (rows - 1)) / rows;
		const tileWidth = Math.max(0, Math.min(maxWidth, maxHeight * aspect));
		if (tileWidth > best.tileWidth) {
			best = { cols, rows, tileWidth, tileHeight: tileWidth / aspect };
		}
	}
	return best;
}

/** Landscape stages use 16:9 tiles; portrait phones use 3:4 so faces fill the screen. */
export const tileAspect = (width: number, height: number): number =>
	width >= height ? 16 / 9 : 3 / 4;
