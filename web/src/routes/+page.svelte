<script lang="ts">
	import { createMeeting, type ApiError, type NewMeeting } from '#lib/api.ts';
	import CopyButton from '#lib/components/CopyButton.svelte';
	import type { RecordingPolicy } from '#lib/protocol.ts';
	import {
		forgetCreateSecret,
		loadCreateSecret,
		saveCreateSecret,
		saveHostToken
	} from '#lib/storage.ts';

	type HostState =
		| { kind: 'idle' }
		| { kind: 'creating' }
		| { kind: 'created'; meeting: NewMeeting }
		| { kind: 'failed'; error: ApiError };

	let hostState = $state.raw<HostState>({ kind: 'idle' });
	/** The Worker's create secret; asked for once, then kept in this browser. */
	let secret = $state(loadCreateSecret() ?? '');
	let askSecret = $state(loadCreateSecret() === null);
	let policy = $state<RecordingPolicy>('fromStart');

	const origin = window.location.host;

	async function start(event: SubmitEvent) {
		event.preventDefault();
		const key = secret.trim();
		if (key === '') return;
		hostState = { kind: 'creating' };
		const result = await createMeeting(key, policy);
		if (!result.ok) {
			if (result.error.kind === 'unauthorized') {
				forgetCreateSecret();
				secret = '';
				askSecret = true;
			}
			hostState = { kind: 'failed', error: result.error };
			return;
		}
		saveCreateSecret(key);
		askSecret = false;
		saveHostToken(result.value.meeting, result.value.hostToken);
		hostState = { kind: 'created', meeting: result.value };
	}

	function describe(error: ApiError): string {
		switch (error.kind) {
			case 'unauthorized':
				return 'That create secret is wrong. Only the owner can start meetings. Enter it again.';
			case 'rate-limited':
				return 'Too many new meetings in a short time. Wait a minute, then try again.';
			case 'network':
				return 'The server could not be reached. Check your connection, then try again.';
			case 'server':
				return `The server could not create a meeting (error ${error.status}). Try again.`;
			case 'malformed':
				return 'The server sent a reply this page does not understand. Try again.';
		}
	}
</script>

<svelte:head>
	<title>meet-felix</title>
</svelte:head>

<main>
	<p class="brand">meet-felix</p>

	{#if hostState.kind === 'created'}
		{@const meeting = hostState.meeting.meeting}
		<section class="created" aria-labelledby="link-heading">
			<h1 id="link-heading" class="visually-hidden">Your meeting link</h1>
			<p class="lede">
				Send this link to the people you are meeting. It works until the meeting ends.
			</p>
			<p class="link">
				<span class="host">{origin}/</span>{#each meeting.split('-') as word, i (i)}{#if i > 0}<span
							class="dash">-</span
						>{/if}<span class="word">{word}</span>{/each}
			</p>
			<div class="actions">
				<a class="button primary" href="/{meeting}">Join now</a>
				<CopyButton text="{window.location.origin}/{meeting}" />
			</div>
		</section>
	{:else}
		<section aria-labelledby="start-heading">
			<h1 id="start-heading">Start a meeting</h1>
			<p class="lede">
				You get a three-word link that anyone can open on a phone, tablet or computer. Meetings are
				recorded with everyone's consent and turned into a transcript afterwards.
			</p>
			<form class="create" onsubmit={start}>
				<fieldset>
					<legend>Recording</legend>
					<label class="choice">
						<input type="radio" name="policy" value="fromStart" bind:group={policy} />
						<span>
							<strong>Record from the start</strong>
							<span class="hint">Everyone accepts recording when they join.</span>
						</span>
					</label>
					<label class="choice">
						<input type="radio" name="policy" value="hostDecides" bind:group={policy} />
						<span>
							<strong>Decide during the meeting</strong>
							<span class="hint">Nothing is recorded until a host starts it.</span>
						</span>
					</label>
				</fieldset>
				{#if askSecret}
					<label class="field">
						<span>Create secret</span>
						<input type="password" bind:value={secret} autocomplete="current-password" required />
						<span class="hint">Asked once; this browser remembers it.</span>
					</label>
				{/if}
				<div class="actions">
					<button
						class="button primary"
						type="submit"
						disabled={hostState.kind === 'creating' || secret.trim() === ''}
					>
						{hostState.kind === 'creating' ? 'Creating…' : 'New meeting'}
					</button>
				</div>
			</form>
			{#if hostState.kind === 'failed'}
				<p class="error" role="alert">{describe(hostState.error)}</p>
			{/if}
		</section>
	{/if}
</main>

<style>
	main {
		display: grid;
		grid-template-rows: auto 1fr;
		gap: 3rem;
		min-height: 100dvh;
		max-width: 52rem;
		padding: var(--gutter);
		padding-bottom: calc(var(--gutter) * 2);
	}

	.brand {
		color: var(--lichen);
		font-weight: 650;
		letter-spacing: 0.01em;
	}

	section {
		display: grid;
		align-content: center;
		gap: 1.5rem;
		padding-bottom: 10dvh;
	}

	h1 {
		font-size: var(--step-3);
		line-height: 1.1;
		font-weight: 700;
	}

	.lede {
		max-width: 36rem;
		color: var(--lichen);
		font-size: var(--step-1);
		line-height: 1.45;
	}

	/* The link is the one loud element: big enough to read aloud across a room. */
	.link {
		font-size: var(--step-4);
		line-height: 1.05;
		font-weight: 750;
		letter-spacing: -0.02em;
		overflow-wrap: anywhere;
	}

	.host {
		display: block;
		font-size: var(--step-1);
		font-weight: 500;
		letter-spacing: 0;
		color: var(--lichen);
		margin-bottom: 0.4rem;
	}

	.dash {
		color: var(--reed-strong);
	}

	.word {
		color: var(--lamp);
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem;
	}

	.create {
		display: grid;
		gap: 1.25rem;
		max-width: 36rem;
	}

	fieldset {
		display: grid;
		gap: 0.5rem;
		margin: 0;
		padding: 0;
		border: 0;
	}

	legend {
		margin-bottom: 0.5rem;
		color: var(--lichen);
		font-size: var(--step--1);
	}

	.choice {
		display: flex;
		align-items: flex-start;
		gap: 0.75rem;
		padding: 0.875rem 1rem;
		border-radius: var(--radius-s);
		background: var(--water);
		box-shadow: inset 0 0 0 1px var(--reed);
		cursor: pointer;
	}

	.choice:has(input:checked) {
		box-shadow: inset 0 0 0 2px var(--lamp);
	}

	.choice input {
		margin-top: 0.3rem;
		accent-color: var(--lamp);
	}

	.choice strong {
		display: block;
		font-weight: 650;
	}

	.hint {
		color: var(--lichen);
		font-size: var(--step--1);
	}

	.field {
		display: grid;
		gap: 0.375rem;
		font-size: var(--step--1);
		color: var(--lichen);
	}

	.field input {
		min-height: 48px;
		padding: 0 0.875rem;
		border: 1px solid var(--reed);
		border-radius: var(--radius-s);
		background: var(--water);
		font-size: var(--step-0);
		color: var(--fog);
	}

	.field input:focus-visible {
		border-color: var(--lamp);
	}

	.error {
		max-width: 36rem;
		padding: 0.875rem 1rem;
		border-left: 3px solid var(--signal);
		background: var(--water);
	}
</style>
