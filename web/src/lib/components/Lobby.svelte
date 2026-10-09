<script lang="ts">
	import type { MeetingId } from '../protocol.ts';
	import type { Session } from '../session.svelte.ts';

	/** Shown from pressing Join until the room lets us in. */
	let { meeting, session }: { meeting: MeetingId; session: Session } = $props();

	let waiting = $derived(session.stage.kind === 'waiting');
	let reconnecting = $derived(
		session.connection.kind === 'waiting' ||
			(session.connection.kind === 'connecting' && session.connection.attempt > 1)
	);
</script>

<main>
	<p class="meeting">{meeting}</p>
	{#if waiting}
		<h1>Waiting for the host</h1>
		<p>The meeting is locked. A host lets you in, and you join as soon as they do.</p>
	{:else}
		<h1>Joining…</h1>
	{/if}
	{#if reconnecting}
		<p class="connection" role="status">Connection lost. Reconnecting…</p>
	{/if}
	<div class="actions">
		<button class="button quiet" type="button" onclick={() => session.leave()}>Leave</button>
	</div>
</main>

<style>
	main {
		display: grid;
		align-content: center;
		gap: 1.25rem;
		min-height: 100dvh;
		max-width: 40rem;
		padding: var(--gutter);
	}

	.meeting {
		color: var(--lichen);
		font-weight: 650;
	}

	h1 {
		font-size: var(--step-2);
		line-height: 1.15;
	}

	p {
		color: var(--lichen);
	}

	.connection {
		color: var(--lamp);
	}

	.actions {
		display: flex;
		gap: 0.75rem;
	}
</style>
