<script lang="ts">
	import { onMount } from 'svelte';
	import { createMeeting, type ApiError } from '#lib/api.ts';
	import { describeApiError, type ApiErrorWording } from '#lib/api-messages.ts';
	import CopyButton from '#lib/components/CopyButton.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import LoginForm from '#lib/components/LoginForm.svelte';
	import ShareButton from '#lib/components/ShareButton.svelte';
	import { OwnerSession } from '#lib/owner-session.svelte.ts';
	import { meetingWords, type MeetingId } from '#lib/protocol.ts';
	import { dropLegacySecrets } from '#lib/storage.ts';

	type HostState =
		| { kind: 'idle' }
		| { kind: 'creating' }
		| { kind: 'created'; meeting: MeetingId }
		| { kind: 'failed'; error: ApiError };

	const owner = new OwnerSession();
	let hostState = $state.raw<HostState>({ kind: 'idle' });
	/** On: `fromStart`. Off: `hostDecides`. */
	let record = $state(true);

	onMount(() => {
		dropLegacySecrets();
		void owner.check();
	});

	async function start(event: SubmitEvent) {
		event.preventDefault();
		hostState = { kind: 'creating' };
		const result = await createMeeting(record ? 'fromStart' : 'hostDecides');
		if (result.ok) {
			hostState = { kind: 'created', meeting: result.value };
		} else if (result.error.kind === 'unauthorized') {
			// The session expired since the page loaded: log in again, then start over.
			hostState = { kind: 'idle' };
			owner.rejected();
		} else {
			hostState = { kind: 'failed', error: result.error };
		}
	}

	async function logOut() {
		await owner.logout();
		if (!owner.signedIn) hostState = { kind: 'idle' };
	}

	const WORDING: ApiErrorWording = {
		action: 'create a meeting',
		rateLimited: 'new meetings',
		unauthorized: 'Your login has expired. Log in again.'
	};
</script>

<svelte:head>
	<title>meet-felix</title>
</svelte:head>

<main>
	<header>
		<p class="brand">meet-felix</p>
		{#if owner.state.kind === 'signed-in'}
			<button
				class="logout"
				type="button"
				onclick={logOut}
				disabled={owner.state.logout.kind === 'sending'}
			>
				Log out
			</button>
		{/if}
	</header>

	{#if owner.state.kind === 'signed-in' && owner.state.logout.kind === 'failed'}
		<p class="error" role="alert">
			<Icon name="alert" size={18} />You are still logged in: the server could not be reached.
		</p>
	{/if}

	{#if owner.state.kind === 'checking'}
		<p class="hint" role="status">Checking your login…</p>
	{:else if owner.state.kind === 'signed-out'}
		<section class="login" aria-labelledby="login-heading">
			<h1 id="login-heading">Log in</h1>
			<p class="hint">Only the owner starts meetings here. To join a meeting, open its link.</p>
			<LoginForm {owner} />
		</section>
	{:else if hostState.kind === 'created'}
		{@const meeting = hostState.meeting}
		{@const url = `${window.location.origin}/${meeting}`}
		<section class="created" aria-labelledby="link-heading">
			<h1 id="link-heading" class="eyebrow">Your meeting</h1>
			<p class="words">
				{#each meetingWords(meeting) as word, i (i)}
					{#if i > 0}<span class="dash">-</span>{/if}{word}
				{/each}
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
				<p class="error" role="alert">
					<Icon name="alert" size={18} />{describeApiError(hostState.error, WORDING)}
				</p>
			{/if}
			<div class="spacer"></div>
			<button class="button primary big" type="submit" disabled={hostState.kind === 'creating'}>
				{hostState.kind === 'creating' ? 'Creating…' : 'New meeting'}
			</button>
			<a class="past" href="/meetings">Past meetings</a>
		</form>
	{/if}
</main>

<style>
	/* One column sized for a phone, centred on larger screens. */
	main {
		display: flex;
		flex-direction: column;
		gap: 2rem;
		min-height: 100dvh;
		max-width: 30rem;
		margin: 0 auto;
		padding: var(--gutter);
		padding-top: calc(var(--gutter) + env(safe-area-inset-top));
		padding-bottom: calc(var(--gutter) + env(safe-area-inset-bottom));
	}

	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
	}

	.brand {
		color: var(--lichen);
		font-weight: 650;
		letter-spacing: 0.01em;
	}

	/* Quiet: logging out is rare, and a session lasts 30 days. */
	.logout {
		min-height: var(--touch);
		padding: 0 0.25rem;
		border: 0;
		background: none;
		color: var(--lichen);
		font-size: var(--step--1);
		text-decoration: underline;
		text-underline-offset: 0.2em;
		cursor: pointer;
	}

	.logout:hover {
		color: var(--fog);
	}

	.login {
		display: grid;
		gap: 1.25rem;
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

	.past {
		align-self: center;
		padding: 0.75rem 1rem;
		color: var(--lichen);
		text-underline-offset: 0.2em;
	}

	.past:hover {
		color: var(--fog);
	}
	/* Tablets and desktops: the content sits in the middle instead of stretching to the bottom.
	   Last in the file, because it overrides `.create`, `.created` and `.spacer` at equal specificity. */
	@media (min-width: 761px) {
		main {
			justify-content: center;
		}

		.create,
		.created {
			flex: 0 1 auto;
		}

		.spacer {
			flex: 0 0 1rem;
		}
	}
</style>
