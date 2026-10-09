<script lang="ts">
	import type { LocalMedia } from '../local-media.svelte.ts';
	import type { MeetingId, Participant } from '../protocol.ts';
	import type { Session } from '../session.svelte.ts';
	import ControlBar from './ControlBar.svelte';
	import Stage from './Stage.svelte';
	import type { TileModel } from './Tile.svelte';

	let {
		meeting,
		media,
		session,
		name
	}: { meeting: MeetingId; media: LocalMedia; session: Session; name: string } = $props();

	let room = $derived(session.room);

	const isMicOn = (p: Participant) => p.tracks.some((t) => t.source === 'microphone' && !t.muted);

	let tiles: TileModel[] = $derived.by(() => {
		const self: TileModel[] = [
			{
				key: 'self',
				name: room.me?.name ?? name,
				self: true,
				host: room.isHost,
				source: 'camera',
				stream: media.cameraStream,
				micOn: media.micOn,
				speaking: false
			}
		];
		if (media.screenStream) {
			self.push({
				key: 'self-screen',
				name: 'Your screen',
				self: false,
				host: false,
				source: 'screen',
				stream: media.screenStream,
				micOn: true,
				speaking: false
			});
		}
		const others = room.others.flatMap((p): TileModel[] => {
			const base = {
				name: p.name,
				self: false,
				host: p.role === 'Host',
				stream: null,
				micOn: isMicOn(p),
				speaking: session.activeSpeaker === p.id
			};
			const camera: TileModel = { ...base, key: p.id, source: 'camera' };
			const screen: TileModel[] = p.tracks.some((t) => t.source === 'screen')
				? [{ ...base, key: `${p.id}-screen`, name: `${p.name}’s screen`, source: 'screen' }]
				: [];
			return [camera, ...screen];
		});
		return [...self, ...others];
	});

	/** A shared screen wins over the active speaker. */
	let featured = $derived(
		tiles.find((t) => t.source === 'screen') ?? tiles.find((t) => t.speaking) ?? null
	);

	let connectionNote = $derived.by(() => {
		const c = session.connection;
		if (c.kind === 'connecting') return c.attempt > 1 ? 'Reconnecting…' : 'Connecting…';
		if (c.kind === 'waiting')
			return `Connection lost. Retrying in ${Math.ceil(c.retryInMs / 1000)} s`;
		return null;
	});
</script>

<div class="meeting">
	<header>
		{#if room.recording}
			<p class="recording" role="status">
				<span class="dot" aria-hidden="true"></span>
				Recording
			</p>
		{/if}
		<p class="title">{meeting}</p>
		{#if room.locked}<p class="locked">Locked</p>{/if}
		{#if connectionNote}<p class="connection" role="status">{connectionNote}</p>{/if}
	</header>

	<div class="stage-wrap">
		<Stage {tiles} {featured} />
	</div>

	<div class="notices" aria-live="polite">
		{#if room.isHost}
			{#each session.knocks as knock (knock.id)}
				<div class="toast knock">
					<p>{knock.name} wants to join</p>
					<button class="button primary" type="button" onclick={() => session.admit(knock.id)}>
						Let in
					</button>
				</div>
			{/each}
		{/if}
		{#each session.toasts.items as toast (toast.id)}
			<p class={['toast', toast.tone]}>{toast.text}</p>
		{/each}
	</div>

	<ControlBar {media} {session} />
</div>

<style>
	.meeting {
		display: grid;
		grid-template-rows: auto minmax(0, 1fr) auto;
		height: 100dvh;
		overflow: hidden;
	}

	header {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem 1rem;
		min-height: 3.25rem;
		padding: calc(0.5rem + env(safe-area-inset-top)) var(--gutter) 0.5rem;
		font-size: var(--step--1);
	}

	.title {
		color: var(--lichen);
		font-weight: 650;
	}

	/* Always visible while recording runs; nothing else on screen uses this red. */
	.recording {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.25rem 0.75rem;
		border-radius: 999px;
		background: var(--signal);
		color: var(--signal-ink);
		font-weight: 700;
	}

	.dot {
		width: 0.5rem;
		height: 0.5rem;
		border-radius: 50%;
		background: currentColor;
		animation: pulse 2s ease-in-out infinite;
	}

	@keyframes pulse {
		50% {
			opacity: 0.35;
		}
	}

	.locked {
		color: var(--lamp);
	}

	.connection {
		margin-left: auto;
		color: var(--lamp);
	}

	.stage-wrap {
		min-height: 0;
		padding: 0 var(--gutter);
	}

	.notices {
		position: fixed;
		top: calc(3.75rem + env(safe-area-inset-top));
		left: 50%;
		translate: -50% 0;
		display: grid;
		gap: 0.5rem;
		width: min(26rem, calc(100vw - 32px));
		z-index: 10;
		pointer-events: none;
	}

	.toast {
		padding: 0.75rem 1rem;
		border-radius: var(--radius-s);
		background: var(--water);
		box-shadow:
			inset 0 0 0 1px var(--reed),
			0 8px 24px rgb(0 0 0 / 0.35);
		pointer-events: auto;
	}

	.toast.alert {
		box-shadow:
			inset 3px 0 0 var(--signal),
			inset 0 0 0 1px var(--reed),
			0 8px 24px rgb(0 0 0 / 0.35);
	}

	.knock {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
	}

	.knock .button {
		min-height: 44px;
		padding: 0 1rem;
	}
</style>
