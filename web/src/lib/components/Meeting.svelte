<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import type { LocalMedia } from '../local-media.svelte.ts';
	import { AudioLevels } from '../media/audio-levels.svelte.ts';
	import { isLive, streamFor } from '../media/remote.ts';
	import type { Voices } from '../media/remote-audio.svelte.ts';
	import type { OwnerSession } from '../owner-session.svelte.ts';
	import {
		trackKey,
		type MeetingId,
		type ParticipantId,
		type ParticipantView,
		type TrackKind
	} from '../protocol.ts';
	import type { Session } from '../session.svelte.ts';
	import ControlBar from './ControlBar.svelte';
	import Stage from './Stage.svelte';
	import type { TileModel } from './Tile.svelte';
	import Toasts from './Toasts.svelte';

	let {
		meeting,
		media,
		session,
		owner
	}: { meeting: MeetingId; media: LocalMedia; session: Session; owner: OwnerSession } = $props();

	/** Voice levels for the speaking ring, keyed like the camera tiles. */
	const SELF = 'self';
	// A meeting screen belongs to one session for its whole life, so its context never changes.
	const levels = new AudioLevels(untrack(() => session.audio.context));
	const levelOf = (key: string) => levels.levels[key] ?? 0;

	/** The participant a host is about to remove; the confirmation dialog is open while set. */
	let removing = $state.raw<ParticipantView | null>(null);

	/**
	 * The stream of `p`'s `kind` track, or `null` to show the initials. The room view decides
	 * whether `p` publishes `kind`, so a camera turned off switches the tile with the room's
	 * `State`, before the SFU closes the received track.
	 */
	function remoteStream(p: ParticipantView, kind: TrackKind): MediaStream | null {
		if (!p.tracks.includes(kind)) return null;
		const track = session.remote.get(trackKey({ owner: p.id, kind }))?.track;
		return track && isLive(track) ? streamFor(track) : null;
	}

	/** Who the host sees marked "Not recorded" (`Session.awaitingConsent`). */
	let unrecorded: ReadonlySet<ParticipantId> = $derived(
		new Set(session.awaitingConsent.map((p) => p.id))
	);

	let tiles: TileModel[] = $derived.by(() => {
		const self: TileModel[] = [
			{
				key: SELF,
				name: session.me?.name ?? 'You',
				self: true,
				host: session.isHost,
				source: 'camera',
				stream: media.cameraStream,
				micOn: media.micOn,
				unrecorded: false,
				reconnecting: false,
				level: () => levelOf(SELF),
				layerTrack: null,
				actions: null
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
				unrecorded: false,
				reconnecting: false,
				level: () => 0,
				layerTrack: null,
				actions: null
			});
		}
		const others = session.others.flatMap((p): TileModel[] => {
			const camera = remoteStream(p, 'video');
			const screen = remoteStream(p, 'screen');
			const base = { name: p.name, self: false, host: p.role === 'host', actions: null };
			return [
				{
					...base,
					key: String(p.id),
					source: 'camera',
					stream: camera,
					micOn: p.tracks.includes('audio') && !p.muted,
					unrecorded: unrecorded.has(p.id),
					reconnecting: p.reconnecting,
					level: () => levelOf(String(p.id)),
					layerTrack: camera ? { owner: p.id, kind: 'video' } : null,
					actions: session.isHost
						? {
								mute: () => session.send({ type: 'mute', who: p.id }),
								remove: () => (removing = p)
							}
						: null
				},
				...(screen
					? [
							{
								...base,
								key: `${p.id}-screen`,
								name: `${p.name}’s screen`,
								source: 'screen' as const,
								stream: screen,
								micOn: true,
								unrecorded: false,
								reconnecting: false,
								level: () => 0,
								layerTrack: null
							}
						]
					: [])
			];
		});
		return [...self, ...others];
	});

	/**
	 * What each other participant says. Everyone in the room keeps an entry, `null` while their
	 * audio is not received, so their `<audio>` element outlasts a session reset (D33).
	 */
	let heard: Voices = $derived(
		new Map(session.others.map((p) => [p.id, remoteStream(p, 'audio')]))
	);
	$effect(() => session.audio.sync(heard));

	/** Every voice to measure: our microphone and each received audio track. */
	let voices: ReadonlyMap<string, MediaStreamTrack> = $derived(
		new Map([
			...(media.tracks.audio ? [[SELF, media.tracks.audio] as const] : []),
			...[...heard].flatMap(([id, stream]) =>
				(stream?.getAudioTracks() ?? []).map((track) => [String(id), track] as const)
			)
		])
	);

	// Web Audio lives outside Svelte: keep its meters in step with the tracks.
	$effect(() => levels.sync(voices));
	onMount(() => () => levels.dispose());

	/** A shared screen gets the large tile. */
	let featured = $derived(tiles.find((t) => t.source === 'screen') ?? null);

	/** The meeting's root element: what goes fullscreen while a screen is expanded. */
	let root: HTMLDivElement | undefined = $state();

	/**
	 * The shared screen shown alone, with the control bar below it, or `null`. A screen is known
	 * by its tile key and its stream, so a share that ends never comes back expanded, not even
	 * when the SFU reuses its slot for another track. In-page only where elements cannot go
	 * fullscreen (iPhone Safari): the meeting already fills the viewport there.
	 */
	let expandedRef = $state.raw<{ key: string; stream: MediaStream } | null>(null);
	let expanded = $derived.by(() => {
		const ref = expandedRef;
		if (ref === null) return null;
		return (
			tiles.find((t) => t.source === 'screen' && t.key === ref.key && t.stream === ref.stream) ??
			null
		);
	});

	/** Expands `screen`, or goes back with `null`; fullscreen follows where the browser has it. */
	function expand(screen: TileModel | null) {
		expandedRef = screen?.stream ? { key: screen.key, stream: screen.stream } : null;
		if (screen && document.fullscreenEnabled && root && document.fullscreenElement !== root) {
			root.requestFullscreen().catch(() => {});
		}
	}

	// Nothing expanded any more (Exit, or the share ended): forget the screen, leave fullscreen.
	$effect(() => {
		if (expanded !== null) return;
		if (expandedRef !== null) expandedRef = null;
		if (root && document.fullscreenElement === root) document.exitFullscreen().catch(() => {});
	});

	/**
	 * Leaving fullscreen (Esc, a system gesture) also leaves the expanded view. Fullscreen that
	 * arrives after the view already collapsed (a quick Exit) is left again.
	 */
	function onFullscreenChange() {
		if (document.fullscreenElement !== root) expandedRef = null;
		else if (expanded === null) document.exitFullscreen().catch(() => {});
	}

	function modal(dialog: HTMLDialogElement) {
		dialog.showModal();
		return () => dialog.close();
	}

	function confirmRemove(who: ParticipantId) {
		session.send({ type: 'kick', who });
		removing = null;
	}
</script>

<svelte:document onfullscreenchange={onFullscreenChange} />

<div bind:this={root} class={['meeting', expanded && 'expanded']} data-testid="meeting">
	<!-- Expanded, the header gives way to the screen; only the recording badge stays (D15). -->
	{#if !expanded || session.room?.recording}
		<header>
			{#if session.room?.recording}
				<p class="recording" role="status">
					<span class="dot" aria-hidden="true"></span>
					Recording
				</p>
			{/if}
			{#if !expanded}
				<p class="title">{meeting}</p>
				{#if session.room?.locked}<p class="locked">Locked</p>{/if}
			{/if}
		</header>
	{/if}

	<div class="stage-wrap">
		<Stage {tiles} {featured} {expanded} layers={session.layers} onexpand={expand} />
	</div>

	<!-- Notices hang above the bar, over the stage's lower edge, so the tiles never jump. -->
	<div class="dock">
		<Toasts {session} />
		<ControlBar {media} {session} {owner} />
	</div>
</div>

{#if removing}
	{@const who = removing}
	<dialog
		class="dialog"
		aria-labelledby="remove-title"
		{@attach modal}
		onclose={() => (removing = null)}
	>
		<h2 id="remove-title">Remove {who.name} from the meeting?</h2>
		<p>They can join again with the link unless you lock the meeting first.</p>
		<div class="dialog-actions">
			<button class="button quiet" type="button" onclick={() => (removing = null)}>Cancel</button>
			<button class="button danger" type="button" onclick={() => confirmRemove(who.id)}>
				Remove
			</button>
		</div>
	</dialog>
{/if}

<style>
	/* Named areas, so the stage keeps its row while the header is left out. */
	.meeting {
		display: grid;
		grid-template:
			'header' auto
			'stage' minmax(0, 1fr)
			'dock' auto;
		height: 100dvh;
		overflow: hidden;
		background: var(--lake);
	}

	header {
		grid-area: header;
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

	.stage-wrap {
		grid-area: stage;
		min-height: 0;
		padding: 0 var(--gutter);
	}

	/* The expanded screen fills the stage edge to edge, on black; only the phone's safe areas stay. */
	.expanded .stage-wrap {
		padding: env(safe-area-inset-top) env(safe-area-inset-right) 0 env(safe-area-inset-left);
		background: #000;
	}

	.dock {
		grid-area: dock;
		position: relative;
	}
</style>
