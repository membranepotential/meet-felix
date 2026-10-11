<script lang="ts">
	import type { OwnerSession } from '../owner-session.svelte.ts';
	import { hostOffer } from '../owner-state.ts';
	import type { MeetingId } from '../protocol.ts';
	import type { Session } from '../session.svelte.ts';
	import HostLogin from './HostLogin.svelte';

	/** Shown from pressing Join until the room lets us in. */
	let {
		meeting,
		session,
		owner,
		scheduled
	}: {
		meeting: MeetingId;
		session: Session;
		owner: OwnerSession;
		/** The meeting had not started when the pre-join looked it up: no host has arrived. */
		scheduled: boolean;
	} = $props();

	let waiting = $derived(session.stage.kind === 'waiting');
	let reconnecting = $derived(
		session.connection.kind === 'waiting' ||
			(session.connection.kind === 'connecting' && session.connection.attempt > 1)
	);
	/** The user opened "Join as host". */
	let hostFormOpen = $state(false);
	let offer = $derived(hostOffer(owner.state, hostFormOpen));
</script>

<main>
	<p class="meeting">{meeting}</p>
	{#if session.state.consentAsked}
		<!-- Seen under the consent dialog: a recording runs and we have not accepted it. -->
		<h1>Recording consent</h1>
		<p>Accept the recording to join.</p>
	{:else if waiting && scheduled}
		<h1>Waiting for the host</h1>
		<p>The meeting has not started yet. You join as soon as the host arrives.</p>
	{:else if waiting}
		<h1>Waiting to be let in</h1>
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
	{#if waiting}
		<!-- A new socket carries the owner's session cookie, and the room makes us the host. -->
		{#if offer.kind === 'link'}
			<button class="host-link" type="button" onclick={() => (hostFormOpen = true)}>
				Join as host
			</button>
		{:else if offer.kind === 'host'}
			<button class="host-link" type="button" onclick={() => session.reconnect()}>
				Join as host
			</button>
		{:else if offer.kind === 'form'}
			<div class="host-login">
				<HostLogin {owner} onlogin={() => session.reconnect()} />
			</div>
		{/if}
	{/if}
</main>

<style>
	main {
		display: grid;
		align-content: center;
		gap: 1.25rem;
		min-height: 100dvh;
		max-width: 40rem;
		margin: 0 auto;
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

	.host-login {
		max-width: 26rem;
	}

	/* Quiet on purpose: only the owner needs it. */
	.host-link {
		justify-self: start;
		min-height: var(--touch);
		padding: 0;
		border: 0;
		background: none;
		color: var(--lichen);
		font-size: var(--step--1);
		text-decoration: underline;
		text-underline-offset: 0.2em;
		cursor: pointer;
	}

	.host-link:hover {
		color: var(--fog);
	}
</style>
