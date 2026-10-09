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
		<div class={['spotlight', landscape ? 'beside' : 'below']}>
			<div class="featured"><Tile tile={featured} {layers} /></div>
			{#if rest.length > 0}
				<ul class="strip" aria-label="Other participants">
					{#each rest as tile (tile.key)}
						<li><Tile {tile} {layers} compact /></li>
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

	/* Portrait: big tile on top, a scrolling row of small tiles below. */
	.spotlight.below {
		--strip: clamp(5.5rem, 16vh, 8rem);
		grid-template-rows: minmax(0, 1fr) var(--strip);
	}

	.spotlight.below .strip {
		display: grid;
		grid-auto-flow: column;
		grid-auto-columns: max-content;
		gap: 8px;
		overflow-x: auto;
	}

	.spotlight.below .strip li {
		width: calc(var(--strip) * 3 / 4);
		height: var(--strip);
	}

	.featured {
		min-width: 0;
		min-height: 0;
	}
</style>
