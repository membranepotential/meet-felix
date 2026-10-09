<script lang="ts">
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { demoWelcome } from '../demo.ts';
	import { LocalMedia } from '../local-media.svelte.ts';
	import type { MeetingId } from '../protocol.ts';
	import { Session } from '../session.svelte.ts';
	import { loadDisplayName, loadHostToken, saveDisplayName } from '../storage.ts';
	import CopyButton from './CopyButton.svelte';
	import Meeting from './Meeting.svelte';
	import PreJoin from './PreJoin.svelte';

	let { meeting }: { meeting: MeetingId } = $props();

	/**
	 * Until the Worker exposes meeting info, assume every meeting is recorded
	 * (recording is on by default, docs/recording/README.md, D15).
	 */
	const RECORDED = true;

	/** Dev only: `?demo=N` fills the room without a server (see `demo.ts`). */
	const demo = import.meta.env.DEV ? Number(page.url.searchParams.get('demo')) || null : null;

	const media = new LocalMedia();
	let name = $state(loadDisplayName());
	let session = $state.raw<Session | null>(null);

	onMount(() => {
		void media.start();
		return () => {
			session?.dispose();
			media.dispose();
		};
	});

	function join() {
		const trimmed = name.trim();
		saveDisplayName(trimmed);
		const next = new Session(
			meeting,
			{ name: trimmed, consent: RECORDED, hostToken: loadHostToken(meeting) },
			// Release camera and mic as soon as the stay is over, not only when the page closes.
			{ onEnd: () => media.dispose(), offline: demo !== null }
		);
		next.start();
		if (demo !== null) {
			next.receive(demoWelcome(meeting, trimmed, demo));
			if (page.url.searchParams.has('speaker'))
				next.activeSpeaker = next.room.others[0]?.id ?? null;
		}
		session = next;
	}
</script>

{#if session === null}
	<PreJoin {meeting} {media} recorded={RECORDED} bind:name onjoin={join} />
{:else if session.end === null}
	<Meeting {meeting} {media} {session} {name} />
{:else}
	<main class="message">
		{#if session.end.kind === 'left'}
			<h1>You left the meeting</h1>
			<div class="actions">
				<button class="button primary" type="button" onclick={() => window.location.reload()}>
					Rejoin
				</button>
				<a class="button quiet" href="/">Start page</a>
			</div>
		{:else if session.end.kind === 'ended'}
			<h1>The meeting has ended</h1>
			{#if session.end.transcriptUrl}
				<p>
					The transcript will appear at this link once it is ready. Save the link now; this page
					does not show it again.
				</p>
				<p class="transcript">
					<a href={session.end.transcriptUrl}>{session.end.transcriptUrl}</a>
				</p>
				<div class="actions"><CopyButton text={session.end.transcriptUrl} /></div>
			{/if}
		{:else}
			<h1>You are not in the meeting</h1>
			<p>{session.end.reason}</p>
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
