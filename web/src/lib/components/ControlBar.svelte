<script lang="ts">
	import type { LocalMedia } from '../local-media.svelte.ts';
	import { canShareScreen } from '../media/capture.ts';
	import type { Session } from '../session.svelte.ts';
	import Icon from './Icon.svelte';

	let { media, session }: { media: LocalMedia; session: Session } = $props();

	const shareable = canShareScreen();
	let hostOpen = $state(false);
	let confirmEnd: HTMLDialogElement | undefined = $state();

	let locked = $derived(session.room?.locked ?? false);
	let recording = $derived(session.room?.recording !== undefined);

	async function toggleShare() {
		if (media.screen) {
			media.stopScreenShare();
			return;
		}
		const shared = await media.startScreenShare();
		if (!shared) session.toasts.show('Screen sharing did not start.');
	}

	function endForEveryone() {
		confirmEnd?.close();
		session.send({ type: 'end' });
	}
</script>

<div class="bar" role="toolbar" aria-label="Meeting controls">
	<button
		type="button"
		class={['control', !media.micOn && 'off']}
		aria-pressed={!media.micOn}
		onclick={() => media.toggleMic()}
		disabled={!media.tracks.audio}
	>
		<Icon name={media.micOn ? 'mic' : 'mic-off'} />
		<span class="label">{media.micOn ? 'Mute' : 'Unmute'}</span>
	</button>

	<button
		type="button"
		class={['control', !media.camOn && 'off']}
		aria-pressed={!media.camOn}
		onclick={() => media.toggleCamera()}
	>
		<Icon name={media.camOn ? 'camera' : 'camera-off'} />
		<span class="label">{media.camOn ? 'Stop video' : 'Start video'}</span>
	</button>

	{#if shareable}
		<button
			type="button"
			class={['control', media.screen && 'active']}
			aria-pressed={media.screen !== null}
			onclick={toggleShare}
		>
			<Icon name="screen" />
			<span class="label">{media.screen ? 'Stop sharing' : 'Share screen'}</span>
		</button>
	{/if}

	{#if session.isHost}
		<button
			type="button"
			class={['control', 'host-toggle', hostOpen && 'active']}
			aria-expanded={hostOpen}
			aria-controls="host-controls"
			onclick={() => (hostOpen = !hostOpen)}
		>
			<Icon name="host" />
			<span class="label">Host</span>
		</button>

		<div id="host-controls" class={['host-group', hostOpen && 'open']}>
			<button
				type="button"
				class={['control', locked && 'active']}
				aria-pressed={locked}
				onclick={() => session.send({ type: 'lock', on: !locked })}
			>
				<Icon name={locked ? 'lock' : 'unlock'} />
				<span class="label">{locked ? 'Unlock' : 'Lock'}</span>
			</button>
			<button
				type="button"
				class={['control', recording && 'recording']}
				aria-pressed={recording}
				onclick={() => session.send({ type: 'setRecording', on: !recording })}
			>
				<Icon name="record" />
				<span class="label">{recording ? 'Stop recording' : 'Record'}</span>
			</button>
			<button type="button" class="control" onclick={() => confirmEnd?.showModal()}>
				<Icon name="end" />
				<span class="label">End for all</span>
			</button>
		</div>
	{/if}

	<button type="button" class="control leave" onclick={() => session.leave()}>
		<Icon name="leave" />
		<span class="label">Leave</span>
	</button>
</div>

<dialog class="dialog" bind:this={confirmEnd} aria-labelledby="end-title">
	<h2 id="end-title">End the meeting for everyone?</h2>
	<p>Everyone is disconnected and the link stops working. The transcript is made afterwards.</p>
	<div class="dialog-actions">
		<button class="button quiet" type="button" onclick={() => confirmEnd?.close()}>Cancel</button>
		<button class="button danger" type="button" onclick={endForEveryone}>End meeting</button>
	</div>
</dialog>

<style>
	.bar {
		position: relative;
		display: flex;
		justify-content: center;
		align-items: flex-start;
		gap: 0.5rem;
		padding: 0.75rem var(--gutter) calc(0.75rem + env(safe-area-inset-bottom));
	}

	.control {
		display: grid;
		justify-items: center;
		align-content: center;
		gap: 0.25rem;
		min-width: var(--touch);
		padding: 0;
		border: 0;
		background: none;
		font-size: 0.75rem;
		color: var(--lichen);
	}

	/* The round face of each control; the label sits under it. */
	.control > :global(svg) {
		box-sizing: content-box;
		padding: 16px;
		border-radius: 50%;
		background: var(--water);
		color: var(--fog);
		box-shadow: inset 0 0 0 1px var(--reed);
		transition: background-color 120ms ease;
	}

	.control:hover > :global(svg) {
		background: var(--reed);
	}

	.control:focus-visible {
		box-shadow: none;
	}

	.control:focus-visible > :global(svg) {
		box-shadow: var(--focus);
	}

	.control.off > :global(svg) {
		background: var(--fog);
		color: var(--lake);
	}

	.control.active > :global(svg) {
		background: var(--lamp);
		color: var(--lamp-ink);
	}

	.control.recording > :global(svg),
	.control.leave > :global(svg) {
		background: var(--signal);
		color: var(--signal-ink);
		box-shadow: none;
	}

	.control:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}

	.label {
		white-space: nowrap;
	}

	.host-group {
		display: flex;
		gap: 0.5rem;
		padding-left: 0.5rem;
		margin-left: 0.25rem;
		border-left: 1px solid var(--reed);
	}

	.host-toggle {
		display: none;
	}

	.leave {
		margin-left: 0.5rem;
	}

	/* Phones: host controls fold into a row that opens above the bar. */
	@media (max-width: 640px) {
		.bar {
			gap: 0.25rem;
		}

		.label {
			font-size: 0.6875rem;
		}

		.host-toggle {
			display: grid;
		}

		.host-group {
			display: none;
			position: absolute;
			bottom: 100%;
			left: 50%;
			translate: -50% 0;
			padding: 0.75rem 1rem;
			margin: 0;
			border: 0;
			border-radius: var(--radius-m);
			background: var(--water);
			box-shadow: inset 0 0 0 1px var(--reed);
		}

		.host-group.open {
			display: flex;
		}

		.leave {
			margin-left: 0;
		}
	}
</style>
