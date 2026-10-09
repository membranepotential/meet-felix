<script lang="ts" module>
	/** What a tile shows. */
	export interface TileView {
		name: string;
		self: boolean;
		host: boolean;
		source: 'camera' | 'screen';
		/** `null` shows the initials; remote streams arrive with the SFU in phase 2. */
		stream: MediaStream | null;
		micOn: boolean;
		speaking: boolean;
	}

	/** A tile in a list; `key` identifies it across updates. */
	export interface TileModel extends TileView {
		key: string;
	}
</script>

<script lang="ts">
	import { showStream } from '../media/video.ts';
	import Icon from './Icon.svelte';

	let { tile, compact = false }: { tile: TileView; compact?: boolean } = $props();

	let initials = $derived(
		tile.name
			.split(/\s+/)
			.filter(Boolean)
			.slice(0, 2)
			.map((part) => part[0]?.toUpperCase())
			.join('')
	);
</script>

<figure class={['tile', tile.speaking && 'speaking', compact && 'compact']}>
	{#if tile.stream}
		<video
			{@attach showStream(tile.stream)}
			class={[tile.source === 'screen' ? 'screen' : 'camera', tile.self && 'mirror']}
		></video>
	{:else if tile.source === 'screen'}
		<div class="avatar" aria-hidden="true"><Icon name="screen" size={32} /></div>
	{:else}
		<div class="avatar" aria-hidden="true">{initials}</div>
	{/if}
	<figcaption>
		{#if !tile.micOn}<span class="muted"><Icon name="mic-off" size={16} /></span>{/if}
		<span class="name">{tile.self ? `${tile.name} (you)` : tile.name}</span>
		{#if tile.host}<span class="visually-hidden">(host)</span>{/if}
	</figcaption>
</figure>

<style>
	.tile {
		position: relative;
		margin: 0;
		width: 100%;
		height: 100%;
		border-radius: var(--radius-m);
		background: var(--water);
		overflow: hidden;
		box-shadow: inset 0 0 0 1px var(--reed);
		container-type: size;
		transition: box-shadow 160ms ease;
	}

	.tile.speaking {
		box-shadow: inset 0 0 0 3px var(--lamp);
	}

	.compact {
		border-radius: var(--radius-s);
	}

	video {
		display: block;
		width: 100%;
		height: 100%;
	}

	.camera {
		object-fit: cover;
	}

	/* Shared screens keep every pixel visible. */
	.screen {
		object-fit: contain;
		background: #08100f;
	}

	.mirror {
		transform: scaleX(-1);
	}

	.avatar {
		position: absolute;
		inset: 0;
		margin: auto;
		display: grid;
		place-items: center;
		width: min(40cqw, 45cqh, 7rem);
		aspect-ratio: 1;
		border-radius: 50%;
		background: var(--reed);
		color: var(--fog);
		font-size: clamp(0.875rem, 16cqh, 2.25rem);
		font-weight: 700;
	}

	figcaption {
		position: absolute;
		left: 0.5rem;
		bottom: 0.5rem;
		display: flex;
		align-items: center;
		gap: 0.375rem;
		max-width: calc(100% - 1rem);
		padding: 0.2rem 0.625rem;
		border-radius: 999px;
		background: rgb(14 28 34 / 0.78);
		font-size: var(--step--1);
		line-height: 1.4;
	}

	.compact figcaption {
		left: 0.25rem;
		bottom: 0.25rem;
		padding: 0.1rem 0.5rem;
		font-size: 0.8125rem;
	}

	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.muted {
		display: inline-flex;
		color: var(--signal);
	}
</style>
