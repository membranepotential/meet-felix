<script lang="ts" module>
	import type { TrackRef } from '../protocol.ts';

	/** What a tile shows. */
	export interface TileView {
		name: string;
		self: boolean;
		host: boolean;
		source: 'camera' | 'screen';
		/** `null` shows the initials (or a screen icon). */
		stream: MediaStream | null;
		micOn: boolean;
		/** The room records without this person's consent; shown only to hosts. */
		unrecorded: boolean;
		/**
		 * Reads how loud this person's voice is now, 0..1 (see `media/speaking.ts`). A function,
		 * so a level change re-renders only the ring and bars, not the tile list (which would
		 * rebind the video).
		 */
		level: () => number;
		/** A received camera track: its simulcast layer follows this tile's width. */
		layerTrack: TrackRef | null;
		/** Host actions on the participant shown; `null` unless we are a host. */
		actions: { mute: () => void; remove: () => void } | null;
	}

	/** A tile in a list; `key` identifies it across updates. */
	export interface TileModel extends TileView {
		key: string;
	}

	/**
	 * `tile`: a grid or featured tile. `strip`: small, next to a shared screen. `chip`: a round
	 * face below a shared screen on a phone; the name is for screen readers only. `expanded`: a
	 * shared screen alone on the stage, edge to edge on black.
	 */
	export type TileVariant = 'tile' | 'strip' | 'chip' | 'expanded';
</script>

<script lang="ts">
	import type { LayerControl } from '../layers.ts';
	import { isSpeaking, levelBars } from '../media/speaking.ts';
	import { observeWidth, showStream } from '../media/video.ts';
	import Icon from './Icon.svelte';

	let {
		tile,
		layers,
		variant = 'tile',
		onexpand
	}: {
		tile: TileView;
		layers: LayerControl;
		variant?: TileVariant;
		/** Asks to show this tile expanded (`true`) or back on the normal stage (`false`). */
		onexpand?: (on: boolean) => void;
	} = $props();

	let level = $derived(tile.level());
	let speaking = $derived(isSpeaking(level));
	let bars = $derived(levelBars(level));
	let label = $derived(tile.self ? `${tile.name} (you)` : tile.name);

	let expanded = $derived(variant === 'expanded');
	/** Only a shared screen expands, and only where the stage offers it. */
	let canExpand = $derived(
		onexpand !== undefined && tile.source === 'screen' && tile.stream !== null
	);

	let initials = $derived(
		tile.name
			.split(/\s+/)
			.filter(Boolean)
			.slice(0, 2)
			.map((part) => part[0]?.toUpperCase())
			.join('')
	);

	/** Reports the tile's width for `track` while the tile exists. */
	function reportWidth(track: TrackRef) {
		return (element: HTMLElement) => {
			const stop = observeWidth((width) => layers.report(track, width))(element);
			return () => {
				stop();
				layers.forget(track);
			};
		};
	}
</script>

<figure
	class={['tile', variant, speaking && 'speaking']}
	title={variant === 'chip' ? label : undefined}
	{@attach tile.layerTrack && reportWidth(tile.layerTrack)}
>
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
	{#if tile.actions && variant === 'tile'}
		<div class="actions">
			<button
				type="button"
				onclick={tile.actions.mute}
				title="Mute {tile.name}"
				disabled={!tile.micOn}
			>
				<Icon name="mic-off" size={18} />
				<span class="visually-hidden">Mute {tile.name}</span>
			</button>
			<button type="button" onclick={tile.actions.remove} title="Remove {tile.name}">
				<Icon name="remove" size={18} />
				<span class="visually-hidden">Remove {tile.name}</span>
			</button>
		</div>
	{/if}
	{#if canExpand}
		<button
			type="button"
			class="expand"
			title={expanded ? 'Exit expanded view' : 'Expand screen'}
			aria-label={expanded ? 'Exit expanded view' : 'Expand screen'}
			onclick={() => onexpand?.(!expanded)}
		>
			<Icon name={expanded ? 'fullscreen-exit' : 'fullscreen'} size={22} />
		</button>
	{/if}
	<figcaption class={[variant === 'chip' && 'visually-hidden']}>
		{#if !tile.micOn}<span class="muted"><Icon name="mic-off" size={16} /></span>{/if}
		{#if speaking}
			<span class="bars" aria-hidden="true">
				{#each bars as height, i (i)}<span style:--h={height}></span>{/each}
			</span>
		{/if}
		<span class="name">{label}</span>
		<!-- Not on strip tiles: their caption is too narrow for the name and the label. -->
		{#if tile.unrecorded && variant !== 'strip'}<span class="unrecorded">Not recorded</span>{/if}
		{#if speaking}<span class="visually-hidden">(speaking)</span>{/if}
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
		container-type: size;
	}

	/* The outline sits above the video, so the speaking ring stays visible on camera tiles. */
	.tile::after {
		content: '';
		position: absolute;
		inset: 0;
		border-radius: inherit;
		box-shadow: inset 0 0 0 1px var(--reed);
		pointer-events: none;
	}

	.strip {
		border-radius: var(--radius-s);
	}

	.chip {
		border-radius: 50%;
		box-shadow: 0 0 0 2px var(--lake);
	}

	.chip .avatar {
		width: 100%;
	}

	/* The active speaker: a fog ring. Lamp stays reserved for the next action, red for recording. */
	.speaking::after {
		box-shadow: inset 0 0 0 3px var(--fog);
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

	/* Expanded: the screen on black, every pixel visible, no rounded frame. */
	.expanded {
		border-radius: 0;
		background: #000;
	}

	.expanded::after {
		display: none;
	}

	.expanded .screen {
		background: #000;
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

	.strip figcaption {
		left: 0.25rem;
		bottom: 0.25rem;
		padding: 0.1rem 0.5rem;
		font-size: 0.8125rem;
	}

	/* Leaves room for the expand button in the other corner. */
	.expand ~ figcaption {
		max-width: calc(100% - 4.5rem);
	}

	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.actions {
		position: absolute;
		top: 0.5rem;
		right: 0.5rem;
		display: flex;
		gap: 0.375rem;
	}

	.actions button {
		display: grid;
		place-items: center;
		width: 44px;
		height: 44px;
		border: 0;
		border-radius: 50%;
		background: rgb(14 28 34 / 0.78);
		color: var(--fog);
	}

	.expand {
		position: absolute;
		right: 0.5rem;
		bottom: 0.5rem;
		display: grid;
		place-items: center;
		width: 48px;
		height: 48px;
		border: 0;
		border-radius: 50%;
		background: rgb(14 28 34 / 0.62);
		color: var(--fog);
		transition: background-color 120ms ease;
	}

	.strip .expand {
		right: 0.25rem;
		bottom: 0.25rem;
		width: 44px;
		height: 44px;
	}

	.actions button:hover,
	.expand:hover {
		background: var(--reed);
	}

	.muted {
		display: inline-flex;
		color: var(--signal);
	}

	.unrecorded {
		flex: none;
		color: var(--lichen);
		font-size: 0.8125rem;
	}

	/* Three level bars, scaled by `levelBars`. */
	.bars {
		display: inline-flex;
		align-items: flex-end;
		gap: 2px;
		flex: none;
		height: 0.8em;
	}

	.bars span {
		width: 3px;
		height: calc(var(--h) * 100%);
		border-radius: 1px;
		background: var(--fog);
		transition: height 100ms linear;
	}
</style>
