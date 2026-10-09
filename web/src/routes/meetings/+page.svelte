<script lang="ts">
	import { onMount } from 'svelte';
	import { listMeetings, type ApiError, type PastMeeting } from '#lib/api.ts';
	import LoginForm from '#lib/components/LoginForm.svelte';
	import { OwnerSession } from '#lib/owner-session.svelte.ts';
	import type { Phase } from '#lib/protocol.ts';

	type ListState =
		| { kind: 'loading' }
		| { kind: 'loaded'; meetings: PastMeeting[] }
		| { kind: 'failed'; error: ApiError };

	const PHASE_LABEL: Record<Phase, string> = {
		scheduled: 'Scheduled',
		live: 'Live',
		ended: 'Ended'
	};

	const owner = new OwnerSession();
	let listState = $state.raw<ListState>({ kind: 'loading' });

	const when = new Intl.DateTimeFormat(undefined, {
		weekday: 'short',
		day: 'numeric',
		month: 'short',
		year: 'numeric',
		hour: '2-digit',
		minute: '2-digit'
	});

	onMount(async () => {
		await owner.check();
		if (owner.signedIn) await load();
	});

	async function load() {
		listState = { kind: 'loading' };
		const result = await listMeetings();
		if (!result.ok && result.error.kind === 'unauthorized') {
			// The session expired since the page loaded: the login form shows again.
			owner.rejected();
			return;
		}
		listState = result.ok
			? { kind: 'loaded', meetings: result.value }
			: { kind: 'failed', error: result.error };
	}

	function describe(error: ApiError): string {
		switch (error.kind) {
			case 'unauthorized':
				return 'Your login has expired. Log in again.';
			case 'rate-limited':
				return 'Too many requests in a short time. Wait a minute, then try again.';
			case 'network':
				return 'The server could not be reached. Check your connection, then try again.';
			case 'server':
				return error.status === 404
					? 'This server cannot list meetings yet. Update the Worker, then try again.'
					: `The server could not list meetings (error ${error.status}). Try again.`;
			case 'malformed':
				return 'The server sent a reply this page does not understand. Try again.';
		}
	}
</script>

<svelte:head>
	<title>Past meetings – meet-felix</title>
</svelte:head>

<main>
	<a class="brand" href="/">meet-felix</a>
	<h1>Past meetings</h1>

	{#if owner.state.kind === 'checking'}
		<p class="hint" role="status">Checking your login…</p>
	{:else if owner.state.kind === 'signed-out'}
		<p class="hint">Your meetings, newest first, with their transcripts. Log in to see them.</p>
		<LoginForm {owner} onlogin={load} />
	{:else if listState.kind === 'loading'}
		<p class="hint" role="status">Loading…</p>
	{:else if listState.kind === 'failed'}
		<p class="error" role="alert">{describe(listState.error)}</p>
		<button class="button quiet" type="button" onclick={load}>Try again</button>
	{:else}
		{#if listState.meetings.length === 0}
			<p class="hint">No meetings yet. Meetings you start appear here.</p>
		{:else}
			<ul class="list">
				{#each listState.meetings as past (`${past.meeting}@${past.createdAt}`)}
					<li class="item">
						<span class="words">{past.meeting}</span>
						<time class="hint" datetime={new Date(past.createdAt).toISOString()}>
							{when.format(past.createdAt)}
						</time>
						<span class="state {past.phase}">{PHASE_LABEL[past.phase]}</span>
						{#if past.transcript}
							<a class="transcript" href="/t/{encodeURIComponent(past.transcript)}">Transcript</a>
						{:else if past.phase === 'ended'}
							<span class="hint transcript">No transcript</span>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		<a class="button quiet" href="/">Start page</a>
	{/if}
</main>

<style>
	main {
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
		min-height: 100dvh;
		max-width: 36rem;
		padding: var(--gutter);
		padding-top: calc(var(--gutter) + env(safe-area-inset-top));
		padding-bottom: calc(var(--gutter) + env(safe-area-inset-bottom));
	}

	main > .button {
		align-self: flex-start;
	}

	.brand {
		color: var(--lichen);
		font-weight: 650;
		text-decoration: none;
	}

	h1 {
		font-size: var(--step-3);
		line-height: 1.1;
		font-weight: 700;
		margin-top: 0.75rem;
	}

	.hint {
		color: var(--lichen);
		font-size: var(--step--1);
	}

	.list {
		display: grid;
		gap: 0.5rem;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	/* Words and date on the left; state and transcript link on the right. */
	.item {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: center;
		gap: 0.125rem 1rem;
		padding: 0.75rem 1rem;
		border-radius: var(--radius-s);
		background: var(--water);
		box-shadow: inset 0 0 0 1px var(--reed);
	}

	.words {
		font-weight: 650;
		overflow-wrap: anywhere;
	}

	.state {
		grid-row: 1;
		grid-column: 2;
		justify-self: end;
		color: var(--lichen);
		font-size: var(--step--1);
	}

	/* A dot marks a meeting that is not over: filled while it runs, hollow before it starts. */
	.state.live,
	.state.scheduled {
		display: inline-flex;
		align-items: center;
		gap: 0.4em;
	}

	.state.live::before,
	.state.scheduled::before {
		content: '';
		width: 0.5em;
		height: 0.5em;
		border-radius: 50%;
	}

	.state.live {
		color: var(--fog);
		font-weight: 650;
	}

	.state.live::before {
		background: var(--lamp);
	}

	.state.scheduled::before {
		box-shadow: inset 0 0 0 1.5px var(--lichen);
	}

	.transcript {
		grid-column: 2;
		justify-self: end;
	}

	a.transcript {
		display: inline-flex;
		align-items: center;
		min-height: 44px;
		color: var(--lamp);
		font-weight: 650;
		text-underline-offset: 0.2em;
	}

	.error {
		padding: 0.875rem 1rem;
		border-left: 3px solid var(--signal);
		background: var(--water);
	}
</style>
