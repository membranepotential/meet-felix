<script lang="ts">
	import { page } from '$app/state';
	import MeetingFlow from '#lib/components/MeetingFlow.svelte';
	import { parseMeetingId } from '#lib/protocol.ts';

	let meeting = $derived(parseMeetingId(page.params.meeting ?? ''));
</script>

<svelte:head>
	<title>{meeting ? `${meeting} – meet-felix` : 'meet-felix'}</title>
</svelte:head>

{#if meeting}
	<!-- A new meeting id means a new stay: fresh media and session. -->
	{#key meeting}
		<MeetingFlow {meeting} />
	{/key}
{:else}
	<main>
		<h1>This is not a meeting link</h1>
		<p>Meeting links end in three words, like {window.location.host}/amber-otter-lamp.</p>
		<a class="button quiet" href="/">Go to the start page</a>
	</main>
{/if}

<style>
	main {
		display: grid;
		align-content: center;
		justify-items: start;
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
</style>
