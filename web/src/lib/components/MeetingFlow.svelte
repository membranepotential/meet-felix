<script lang="ts">
	import { onMount } from 'svelte';
	import { getMeeting, type MeetingInfo } from '../api.ts';
	import { LocalMedia } from '../local-media.svelte.ts';
	import type { MeetingId } from '../protocol.ts';
	import { Session } from '../session.svelte.ts';
	import { loadDisplayName, loadHostToken, saveDisplayName } from '../storage.ts';
	import ConsentDialog from './ConsentDialog.svelte';
	import CopyButton from './CopyButton.svelte';
	import Lobby from './Lobby.svelte';
	import Meeting from './Meeting.svelte';
	import PreJoin, { type PolicyLookup } from './PreJoin.svelte';

	let { meeting }: { meeting: MeetingId } = $props();

	const media = new LocalMedia();
	let name = $state(loadDisplayName());
	let session = $state.raw<Session | null>(null);
	let meetingInfo = $state.raw<
		{ kind: 'loading' } | { kind: 'failed' } | { kind: 'done'; info: MeetingInfo }
	>({ kind: 'loading' });

	let lookup: PolicyLookup = $derived.by(() => {
		if (meetingInfo.kind !== 'done') return meetingInfo;
		const { info } = meetingInfo;
		return info.kind === 'running'
			? { kind: 'known', policy: info.recordingPolicy }
			: { kind: 'failed' };
	});
	/** The link points to no running meeting, so there is nothing to join. */
	let gone = $derived(
		meetingInfo.kind === 'done' && meetingInfo.info.kind !== 'running'
			? meetingInfo.info.kind
			: null
	);

	let end = $derived(session?.end ?? null);
	let transcriptUrl = $derived(
		end?.kind === 'ended' && end.transcript !== null
			? `${window.location.origin}/t/${encodeURIComponent(end.transcript)}`
			: null
	);

	async function lookUp() {
		meetingInfo = { kind: 'loading' };
		const result = await getMeeting(meeting);
		meetingInfo = result.ok ? { kind: 'done', info: result.value } : { kind: 'failed' };
	}

	onMount(() => {
		void media.start();
		void lookUp();
		return () => {
			session?.dispose();
			media.dispose();
		};
	});

	function join(consent: boolean) {
		const trimmed = name.trim();
		saveDisplayName(trimmed);
		const next = new Session(
			meeting,
			{ name: trimmed, consent, hostToken: loadHostToken(meeting) },
			media
		);
		next.start();
		session = next;
	}
</script>

{#if session === null}
	{#if gone !== null}
		<main class="message">
			<h1>{gone === 'ended' ? 'This meeting has ended' : 'This meeting does not exist'}</h1>
			<p>
				Meeting links work only while the meeting runs. Ask the person who invited you for a new
				link.
			</p>
			<div class="actions"><a class="button quiet" href="/">Start page</a></div>
		</main>
	{:else}
		<PreJoin {meeting} {media} {lookup} bind:name onjoin={join} onretry={lookUp} />
	{/if}
{:else if end === null}
	{#if session.stage.kind === 'in'}
		<Meeting {meeting} {media} {session} />
	{:else}
		<Lobby {meeting} {session} />
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
			<h1>The meeting has ended</h1>
			{#if transcriptUrl}
				<p>
					The transcript will appear at this link once it is ready. Save the link now; this page
					does not show it again.
				</p>
				<p class="transcript">
					<a href={transcriptUrl}>{transcriptUrl}</a>
				</p>
				<div class="actions"><CopyButton text={transcriptUrl} /></div>
			{:else}
				<div class="actions"><a class="button quiet" href="/">Start page</a></div>
			{/if}
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
		padding: var(--gutter);
	}

	h1 {
		font-size: var(--step-2);
		line-height: 1.15;
	}

	p {
		color: var(--lichen);
	}

	.transcript a {
		color: var(--lamp);
		overflow-wrap: anywhere;
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem;
	}
</style>
