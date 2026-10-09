<script lang="ts">
	import { onMount } from 'svelte';
	import type { LocalMedia } from '../local-media.svelte.ts';
	import { AudioLevels } from '../media/audio-levels.svelte.ts';
	import { isLive, streamFor } from '../media/remote.ts';
	import { playAudio } from '../media/video.ts';
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

	let { meeting, media, session }: { meeting: MeetingId; media: LocalMedia; session: Session } =
		$props();

	/** Voice levels for the speaking ring, keyed like the camera tiles. */
	const SELF = 'self';
	const levels = new AudioLevels();
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
					micOn: p.tracks.includes('audio'),
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
								level: () => 0,
								layerTrack: null
							}
						]
					: [])
			];
		});
		return [...self, ...others];
	});

	let audio = $derived(
		session.others.flatMap((p) => {
			const stream = remoteStream(p, 'audio');
			return stream ? [{ key: p.id, stream }] : [];
		})
	);

	/** Every voice to measure: our microphone and each received audio track. */
	let voices: ReadonlyMap<string, MediaStreamTrack> = $derived(
		new Map([
			...(media.tracks.audio ? [[SELF, media.tracks.audio] as const] : []),
			...audio.flatMap(({ key, stream }) =>
				stream.getAudioTracks().map((track) => [String(key), track] as const)
			)
		])
	);

	// Web Audio lives outside Svelte: keep its meters in step with the tracks.
	$effect(() => levels.sync(voices));
	onMount(() => () => levels.dispose());

	/** A shared screen gets the large tile. */
	let featured = $derived(tiles.find((t) => t.source === 'screen') ?? null);

	function modal(dialog: HTMLDialogElement) {
		dialog.showModal();
		return () => dialog.close();
	}

	function confirmRemove(who: ParticipantId) {
		session.send({ type: 'kick', who });
		removing = null;
	}
</script>

<div class="meeting">
	<header>
		{#if session.room?.recording}
			<p class="recording" role="status">
				<span class="dot" aria-hidden="true"></span>
				Recording
			</p>
		{/if}
		<p class="title">{meeting}</p>
		{#if session.room?.locked}<p class="locked">Locked</p>{/if}
	</header>

	<div class="stage-wrap">
		<Stage {tiles} {featured} layers={session.layers} />
	</div>

	{#each audio as { key, stream } (key)}
		<audio {@attach playAudio(stream)}></audio>
	{/each}

	<!-- Notices hang above the bar, over the stage's lower edge, so the tiles never jump. -->
	<div class="dock">
		<Toasts {session} />
		<ControlBar {media} {session} />
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

	.stage-wrap {
		min-height: 0;
		padding: 0 var(--gutter);
	}

	.dock {
		position: relative;
	}
</style>
