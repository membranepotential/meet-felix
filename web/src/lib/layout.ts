/** Pure geometry for the participant grid. */

export interface GridFit {
	cols: number;
	rows: number;
	tileWidth: number;
	tileHeight: number;
}

/**
 * The width-to-height ratios a tile may take. Camera video is cropped to fill its tile, so a
 * tile may stretch anywhere inside this range to use the stage's space.
 */
export interface AspectRange {
	min: number;
	max: number;
}

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/**
 * Chooses the column count that gives `count` tiles the largest area inside `width` × `height`,
 * with `gap` pixels between tiles. Each tile fills its grid cell when the cell's ratio lies in
 * `aspect`; otherwise it takes the nearest ratio in range and stays centred in the cell.
 */
export function fitGrid(
	count: number,
	width: number,
	height: number,
	gap: number,
	aspect: AspectRange
): GridFit {
	const n = Math.max(1, count);
	let best: GridFit = { cols: 1, rows: n, tileWidth: 0, tileHeight: 0 };
	for (let cols = 1; cols <= n; cols++) {
		const rows = Math.ceil(n / cols);
		const cellWidth = (width - gap * (cols - 1)) / cols;
		const cellHeight = (height - gap * (rows - 1)) / rows;
		if (cellWidth <= 0 || cellHeight <= 0) continue;
		const ratio = clamp(cellWidth / cellHeight, aspect.min, aspect.max);
		const tileWidth = Math.min(cellWidth, cellHeight * ratio);
		const tileHeight = tileWidth / ratio;
		if (tileWidth * tileHeight > best.tileWidth * best.tileHeight) {
			best = { cols, rows, tileWidth, tileHeight };
		}
	}
	return best;
}

/**
 * Landscape stages keep tiles between 4:3 and 16:9. Portrait stages (phones) allow tall tiles
 * down to 1:2, so a few faces fill the screen from the header to the control bar.
 */
export const tileAspect = (width: number, height: number): AspectRange =>
	width >= height ? { min: 4 / 3, max: 16 / 9 } : { min: 1 / 2, max: 4 / 3 };
