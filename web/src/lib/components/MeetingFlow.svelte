<script lang="ts">
	import { onMount } from 'svelte';
	import { getMeeting, type MeetingInfo } from '../api.ts';
	import { LocalMedia } from '../local-media.svelte.ts';
	import { OwnerSession } from '../owner-session.svelte.ts';
	import type { MeetingId, TranscriptRef } from '../protocol.ts';
	import { Session } from '../session.svelte.ts';
	import { loadDisplayName, saveDisplayName } from '../storage.ts';
	import ConsentDialog from './ConsentDialog.svelte';
	import CopyButton from './CopyButton.svelte';
	import ShareButton from './ShareButton.svelte';
	import Lobby from './Lobby.svelte';
	import Meeting from './Meeting.svelte';
	import PreJoin, { type PolicyLookup } from './PreJoin.svelte';

	let { meeting }: { meeting: MeetingId } = $props();

	const media = new LocalMedia();
	const owner = new OwnerSession();
	let name = $state(loadDisplayName());
	let session = $state.raw<Session | null>(null);
	let meetingInfo = $state.raw<
		{ kind: 'loading' } | { kind: 'failed' } | { kind: 'done'; info: MeetingInfo }
	>({ kind: 'loading' });

	let lookup: PolicyLookup = $derived.by(() => {
		if (meetingInfo.kind !== 'done') return meetingInfo;
		const { info } = meetingInfo;
		return info.kind === 'open'
			? { kind: 'known', policy: info.recordingPolicy, phase: info.phase }
			: { kind: 'failed' };
	});
	/** The link points to no open meeting, so there is nothing to join. */
	let gone = $derived(
		meetingInfo.kind === 'done' && meetingInfo.info.kind !== 'open' ? meetingInfo.info : null
	);
	let scheduled = $derived(lookup.kind === 'known' && lookup.phase === 'scheduled');

	let end = $derived(session?.end ?? null);
	const transcriptBase = `${window.location.origin}/t/`;

	async function lookUp() {
		meetingInfo = { kind: 'loading' };
		const result = await getMeeting(meeting);
		meetingInfo = result.ok ? { kind: 'done', info: result.value } : { kind: 'failed' };
		// Camera and mic go on only for a meeting that can be joined. A press on a pre-join
		// toggle while the lookup ran may have started a capture: a dead link ends it.
		if (!result.ok) return;
		if (result.value.kind === 'open') void media.start();
		else media.dispose();
	}

	onMount(() => {
		void lookUp();
		void owner.check();
		return () => {
			session?.dispose();
			media.dispose();
		};
	});

	function join(consent: boolean) {
		const trimmed = name.trim();
		saveDisplayName(trimmed);
		// The room makes this a host if the socket carries the owner's session cookie (D32).
		const next = new Session(meeting, { name: trimmed, consent }, media);
		next.start();
		session = next;
	}
</script>

<!-- Everyone present when a recorded meeting ends gets the transcript link (D19). -->
{#snippet ended(heading: string, transcript: TranscriptRef | null)}
	<h1>{heading}</h1>
	{#if transcript !== null}
		{@const token = encodeURIComponent(transcript)}
		{@const url = transcriptBase + token}
		<p>
			The transcript will be at this link once it is ready. Save the link now; this page does not
			show it again.
		</p>
		<a class="transcript" href={url} data-testid="transcript-link"
			><span class="host">{transcriptBase}</span>{token}</a
		>
		<div class="actions">
			<ShareButton {url} title="Meeting transcript" primary />
			<CopyButton text={url} />
			<a class="button quiet" href="/">Start page</a>
		</div>
	{:else}
		<div class="actions"><a class="button quiet" href="/">Start page</a></div>
	{/if}
{/snippet}

{#if session === null}
	{#if gone?.kind === 'ended'}
		<main class="message">
			{@render ended('This meeting has ended', gone.transcript)}
		</main>
	{:else if gone?.kind === 'not-found'}
		<main class="message">
			<h1>This meeting does not exist</h1>
			<p>
				Meeting links work only while the meeting runs. Ask the person who invited you for a new
				link.
			</p>
			<div class="actions"><a class="button quiet" href="/">Start page</a></div>
		</main>
	{:else}
		<PreJoin {meeting} {media} {owner} {lookup} bind:name onjoin={join} onretry={lookUp} />
	{/if}
{:else if end === null}
	{#if session.stage.kind === 'in'}
		<Meeting {meeting} {media} {session} {owner} />
	{:else}
		<Lobby {meeting} {session} {owner} {scheduled} />
	{/if}
	{#if session.state.consentAsked}
		<ConsentDialog onaccept={() => session?.acceptRecording()} onleave={() => session?.leave()} />
	{/if}
{:else}
	<main class="message">
		{#if end.kind === 'left'}
			<h1>You left the meeting</h1>
			<div class="actions">
				<button class="button primary" type="button" onclick={() => window.location.reload()}>
					Rejoin
				</button>
				<a class="button quiet" href="/">Start page</a>
			</div>
		{:else if end.kind === 'kicked'}
			<h1>A host removed you from the meeting</h1>
			<div class="actions"><a class="button quiet" href="/">Start page</a></div>
		{:else if end.kind === 'ended'}
			{@render ended('The meeting has ended', end.transcript)}
		{:else}
			<h1>You are not in the meeting</h1>
			<p>{end.reason}</p>
			<div class="actions"><a class="button quiet" href="/">Start page</a></div>
		{/if}
	</main>
{/if}

<style>
	.message {
		display: grid;
		align-content: center;
		gap: 1.25rem;
		min-height: 100dvh;
		max-width: 40rem;
		margin: 0 auto;
		padding: var(--gutter);
	}

	h1 {
		font-size: var(--step-2);
		line-height: 1.15;
	}

	p {
		color: var(--lichen);
	}

	.transcript {
		padding: 0.75rem 1rem;
		border-radius: var(--radius-s);
		background: var(--water);
		box-shadow: inset 0 0 0 1px var(--reed);
		color: var(--fog);
		overflow-wrap: anywhere;
		text-decoration: none;
	}

	.transcript:hover {
		box-shadow: inset 0 0 0 1px var(--reed-strong);
	}

	.host {
		color: var(--lichen);
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem;
	}
</style>
