<script lang="ts">
	import { listMeetings, type ApiError, type PastMeeting } from '#lib/api.ts';
	import PasswordField from '#lib/components/PasswordField.svelte';

	type ListState =
		| { kind: 'idle' }
		| { kind: 'loading' }
		| { kind: 'loaded'; meetings: PastMeeting[] }
		| { kind: 'failed'; error: ApiError };

	let listState = $state.raw<ListState>({ kind: 'idle' });
	/** Typed or filled by the password manager each time; never stored. */
	let password = $state('');

	const when = new Intl.DateTimeFormat(undefined, {
		weekday: 'short',
		day: 'numeric',
		month: 'short',
		year: 'numeric',
		hour: '2-digit',
		minute: '2-digit'
	});

	async function show(event: SubmitEvent) {
		event.preventDefault();
		const key = password.trim();
		if (key === '') return;
		listState = { kind: 'loading' };
		const result = await listMeetings(key);
		listState = result.ok
			? { kind: 'loaded', meetings: result.value }
			: { kind: 'failed', error: result.error };
		if (result.ok) password = '';
	}

	function describe(error: ApiError): string {
		switch (error.kind) {
			case 'unauthorized':
				return 'That password is wrong. Only the owner can see past meetings.';
			case 'rate-limited':
				return 'Too many tries in a short time. Wait a minute, then try again.';
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

	{#if listState.kind === 'loaded'}
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
						<span class={['state', !past.ended && 'live']}>{past.ended ? 'Ended' : 'Live'}</span>
						{#if past.transcript}
							<a class="transcript" href="/t/{encodeURIComponent(past.transcript)}">Transcript</a>
						{:else if past.ended}
							<span class="hint transcript">No transcript</span>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		<a class="button quiet" href="/">Start page</a>
	{:else}
		<form onsubmit={show}>
			<p class="hint">Your meetings, newest first, with their transcripts.</p>
			<PasswordField bind:value={password} />
			{#if listState.kind === 'failed'}
				<p class="error" role="alert">{describe(listState.error)}</p>
			{/if}
			<button
				class="button primary"
				type="submit"
				disabled={listState.kind === 'loading' || password.trim() === ''}
			>
				{listState.kind === 'loading' ? 'Loading…' : 'Show meetings'}
			</button>
		</form>
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

	form {
		display: grid;
		gap: 1.25rem;
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

	.state.live {
		color: var(--fog);
		font-weight: 650;
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
