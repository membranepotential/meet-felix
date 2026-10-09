<script lang="ts">
	import { onMount } from 'svelte';
	import { createMeeting, type ApiError, type NewMeeting } from '#lib/api.ts';
	import CopyButton from '#lib/components/CopyButton.svelte';
	import PasswordField from '#lib/components/PasswordField.svelte';
	import ShareButton from '#lib/components/ShareButton.svelte';
	import { dropStoredCreateSecret, saveHostToken } from '#lib/storage.ts';

	type HostState =
		| { kind: 'idle' }
		| { kind: 'creating' }
		| { kind: 'created'; meeting: NewMeeting }
		| { kind: 'failed'; error: ApiError };

	let hostState = $state.raw<HostState>({ kind: 'idle' });
	/** Typed or filled by the password manager each time; never stored. */
	let password = $state('');
	/** On: `fromStart`. Off: `hostDecides`. */
	let record = $state(true);

	onMount(dropStoredCreateSecret);

	async function start(event: SubmitEvent) {
		event.preventDefault();
		const key = password.trim();
		if (key === '') return;
		hostState = { kind: 'creating' };
		const result = await createMeeting(key, record ? 'fromStart' : 'hostDecides');
		if (!result.ok) {
			hostState = { kind: 'failed', error: result.error };
			return;
		}
		password = '';
		saveHostToken(result.value.meeting, result.value.hostToken);
		hostState = { kind: 'created', meeting: result.value };
	}

	function describe(error: ApiError): string {
		switch (error.kind) {
			case 'unauthorized':
				return 'That password is wrong. Only the owner can start meetings.';
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
		{@const url = `${window.location.origin}/${meeting}`}
		<section class="created" aria-labelledby="link-heading">
			<h1 id="link-heading" class="eyebrow">Your meeting</h1>
			<p class="words">
				{#each meeting.split('-') as word, i (i)}{#if i > 0}<span class="dash">-</span
						>{/if}{word}{/each}
			</p>
			<p class="link"><span class="host">{window.location.origin}/</span>{meeting}</p>
			<p class="hint">The link works until the meeting ends.</p>
			<div class="spacer"></div>
			<div class="pair">
				<CopyButton text={url} />
				<ShareButton {url} title="Join my meeting" />
			</div>
			<a class="button primary big" href="/{meeting}">Join now</a>
		</section>
	{:else}
		<form class="create" onsubmit={start} aria-labelledby="start-heading">
			<h1 id="start-heading">New meeting</h1>
			<PasswordField bind:value={password} />
			<label class="switch">
				<span class="switch-text">
					<strong>Record and transcribe</strong>
					<span class="hint">
						{record
							? 'Everyone agrees when they join. Video is not recorded.'
							: 'Nothing is recorded unless a host starts it during the meeting.'}
					</span>
				</span>
				<input type="checkbox" role="switch" bind:checked={record} />
			</label>
			{#if hostState.kind === 'failed'}
				<p class="error" role="alert">{describe(hostState.error)}</p>
			{/if}
			<div class="spacer"></div>
			<button
				class="button primary big"
				type="submit"
				disabled={hostState.kind === 'creating' || password.trim() === ''}
			>
				{hostState.kind === 'creating' ? 'Creating…' : 'New meeting'}
			</button>
			<a class="past" href="/meetings">Past meetings</a>
		</form>
	{/if}
</main>

<style>
	/* One column sized for a phone; on a desktop it stays a narrow column on the left. */
	main {
		display: flex;
		flex-direction: column;
		gap: 2rem;
		min-height: 100dvh;
		max-width: 30rem;
		padding: var(--gutter);
		padding-top: calc(var(--gutter) + env(safe-area-inset-top));
		padding-bottom: calc(var(--gutter) + env(safe-area-inset-bottom));
	}

	.brand {
		color: var(--lichen);
		font-weight: 650;
		letter-spacing: 0.01em;
	}

	.create,
	.created {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
	}

	/* Keeps the main button in thumb reach at the bottom of a phone screen. */
	.spacer {
		flex: 1;
		min-height: 1rem;
	}

	h1 {
		font-size: var(--step-3);
		line-height: 1.1;
		font-weight: 700;
	}

	h1.eyebrow {
		color: var(--lichen);
		font-size: var(--step-0);
		font-weight: 500;
		line-height: 1.5;
		margin-bottom: -0.75rem;
	}

	/* The words are the one loud element: one line, big enough to read aloud across a room. */
	.words {
		color: var(--lamp);
		font-size: clamp(1.5rem, 7.6vw, 3rem);
		line-height: 1.1;
		font-weight: 750;
		letter-spacing: -0.02em;
		white-space: nowrap;
	}

	.dash {
		color: var(--reed-strong);
	}

	.link {
		padding: 0.75rem 1rem;
		border-radius: var(--radius-s);
		background: var(--water);
		box-shadow: inset 0 0 0 1px var(--reed);
		overflow-wrap: anywhere;
		user-select: all;
	}

	.host {
		color: var(--lichen);
	}

	.hint {
		color: var(--lichen);
		font-size: var(--step--1);
	}

	.pair {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(9rem, 1fr));
		gap: 0.75rem;
	}

	.big {
		width: 100%;
		min-height: 64px;
		font-size: var(--step-1);
	}

	.switch {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 1rem;
		padding: 1rem;
		border-radius: var(--radius-s);
		background: var(--water);
		box-shadow: inset 0 0 0 1px var(--reed);
		cursor: pointer;
	}

	.switch-text {
		display: grid;
		gap: 0.125rem;
	}

	.switch strong {
		font-weight: 650;
	}

	/* A native checkbox drawn as a switch: the lamp track means on. */
	.switch input {
		appearance: none;
		flex: none;
		position: relative;
		width: 52px;
		height: 32px;
		margin: 0;
		border-radius: 999px;
		background: var(--reed);
		cursor: pointer;
		transition: background-color 120ms ease;
	}

	.switch input::after {
		content: '';
		position: absolute;
		top: 4px;
		left: 4px;
		width: 24px;
		height: 24px;
		border-radius: 50%;
		background: var(--fog);
		transition: translate 120ms ease;
	}

	.switch input:checked {
		background: var(--lamp);
	}

	.switch input:checked::after {
		translate: 20px 0;
		background: var(--lamp-ink);
	}

	.error {
		padding: 0.875rem 1rem;
		border-left: 3px solid var(--signal);
		background: var(--water);
	}

	.past {
		align-self: center;
		padding: 0.75rem 1rem;
		color: var(--lichen);
		text-underline-offset: 0.2em;
	}

	.past:hover {
		color: var(--fog);
	}
</style>
