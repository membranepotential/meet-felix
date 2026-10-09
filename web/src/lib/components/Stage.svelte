<script lang="ts">
	import type { LayerControl } from '../layers.ts';
	import { fitGrid, tileAspect } from '../layout.ts';
	import Tile, { type TileModel } from './Tile.svelte';

	/** `featured` (a shared screen) gets the large tile. */
	let {
		tiles,
		featured,
		layers
	}: { tiles: TileModel[]; featured: TileModel | null; layers: LayerControl } = $props();

	const GAP = 8;

	let width = $state(0);
	let height = $state(0);

	let landscape = $derived(width >= height);
	let rest = $derived(featured ? tiles.filter((t) => t.key !== featured.key) : tiles);
	let fit = $derived(fitGrid(tiles.length, width, height, GAP, tileAspect(width, height)));
</script>

<div class="stage" bind:clientWidth={width} bind:clientHeight={height}>
	{#if featured}
		<!-- One featured tile for both orientations: rotating a phone keeps it, and its fullscreen. -->
		<div class={['spotlight', landscape ? 'beside' : 'below']}>
			<div class="featured">
				<div class="screen-box"><Tile tile={featured} {layers} /></div>
			</div>
			{#if rest.length > 0 && landscape}
				<ul class="strip" aria-label="Other participants">
					{#each rest as tile (tile.key)}
						<li><Tile {tile} {layers} variant="strip" /></li>
					{/each}
				</ul>
			{:else if rest.length > 0}
				<ul class="chips" aria-label="Other participants">
					{#each rest as tile (tile.key)}
						<li><Tile {tile} {layers} variant="chip" /></li>
					{/each}
				</ul>
			{/if}
		</div>
	{:else}
		<ul
			class="grid"
			style:--cols={fit.cols}
			style:--tile-w="{fit.tileWidth}px"
			style:--tile-h="{fit.tileHeight}px"
			style:--gap="{GAP}px"
		>
			{#each tiles as tile (tile.key)}
				<li><Tile {tile} {layers} /></li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.stage {
		position: relative;
		width: 100%;
		height: 100%;
		min-height: 0;
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	/* Equal grid: column count and tile size come from `fitGrid`. */
	.grid {
		display: grid;
		grid-template-columns: repeat(var(--cols), var(--tile-w));
		grid-auto-rows: var(--tile-h);
		gap: var(--gap);
		place-content: center;
		height: 100%;
	}

	.spotlight {
		display: grid;
		gap: 8px;
		height: 100%;
	}

	/* Landscape: big tile left, a scrolling column of small tiles right. */
	.spotlight.beside {
		grid-template-columns: minmax(0, 1fr) clamp(8rem, 18vw, 14rem);
	}

	.spotlight.beside .strip {
		display: grid;
		grid-auto-rows: max-content;
		align-content: start;
		gap: 8px;
		overflow-y: auto;
	}

	.spotlight.beside .strip li {
		aspect-ratio: 16 / 9;
	}

	/*
	 * Portrait (phones): the screen as wide as the stage at its top, faces as round chips right
	 * below it. The free space stays at the bottom, where the notices appear.
	 */
	.spotlight.below {
		grid-template-rows: minmax(0, max-content) auto;
		align-content: start;
	}

	.beside .screen-box {
		height: 100%;
	}

	/* A 16:9 box, so the shared screen is not a thin band inside a tall tile. */
	.below .screen-box {
		width: 100%;
		max-height: 100%;
		aspect-ratio: 16 / 9;
	}

	.chips {
		display: flex;
		justify-content: safe center;
		gap: 10px;
		padding: 4px 2px 8px;
		overflow-x: auto;
	}

	.chips li {
		flex: none;
		width: 64px;
		height: 64px;
	}

	.featured {
		min-width: 0;
		min-height: 0;
	}
</style>
