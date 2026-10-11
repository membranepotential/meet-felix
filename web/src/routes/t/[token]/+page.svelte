<script lang="ts">
	import { page } from '$app/state';
	import CopyButton from '#lib/components/CopyButton.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import {
		getTranscript,
		transcriptFile,
		type ApiResult,
		type TranscriptLookup,
		type TranscriptStatus
	} from '#lib/api.ts';
	import { duration, minutes, timeline } from '#lib/transcript.ts';

	/**
	 * The transcript behind a link (D31). The heading is the meeting's words; one sentence
	 * below says when it was and how long, then the job's state in plain sentences (D34), or,
	 * once ready, the downloads, the speakers and the text as paragraphs, one per speaker turn,
	 * with the time of day it was said. Until then the page asks again every ten seconds.
	 */
	const POLL_MS = 10_000;

	let token = $derived(page.params.token ?? '');
	let url = $derived(page.url.href);
	let result = $state<ApiResult<TranscriptLookup> | null>(null);

	$effect(() => {
		const current = token;
		let stopped = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const poll = async () => {
			const next = await getTranscript(current);
			if (stopped) return;
			result = next;
			const settled =
				next.ok &&
				(next.value.status === 'ready' ||
					next.value.status === 'failed' ||
					next.value.status === 'not-found');
			if (!settled) timer = setTimeout(poll, POLL_MS);
		};
		poll();
		return () => {
			stopped = true;
			clearTimeout(timer);
		};
	});

	const clock = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
	const day = new Intl.DateTimeFormat(undefined, { dateStyle: 'long' });
	const dayAndTime = new Intl.DateTimeFormat(undefined, {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	/** "9 October 2026, 14:03, 1 h 5 min." or, while it runs, "Started 9 October 2026, 14:03." */
	function when(status: TranscriptStatus): string {
		const { startedAt } = status;
		const start = `${day.format(startedAt)}, ${clock.format(startedAt)}`;
		return status.status === 'recording'
			? `Started ${start}.`
			: `${start}, ${duration(status.endedAt - startedAt)}.`;
	}
</script>

<svelte:head>
	<title>Transcript – meet-felix</title>
	<meta name="robots" content="noindex" />
	<meta name="referrer" content="no-referrer" />
</svelte:head>

<main>
	<a class="brand" href="/">meet-felix</a>

	{#if result === null}
		<h1>Transcript</h1>
		<p class="note" role="status"><Icon name="info" size={20} />Loading the transcript…</p>
	{:else if !result.ok}
		<h1>Transcript</h1>
		<p class="note alert" role="alert">
			<Icon name="alert" size={20} />The transcript could not be loaded. This page tries again in a
			few seconds.
		</p>
	{:else if result.value.status === 'not-found'}
		<h1>Transcript</h1>
		<p class="note alert" role="alert">
			<Icon name="alert" size={20} />There is no transcript behind this link. It may have expired,
			or the link is incomplete.
		</p>
	{:else}
		{@const status = result.value}
		<header>
			<h1>{status.meeting}</h1>
			<p>{when(status)}</p>
		</header>

		{#if status.status === 'recording'}
			<p class="note" role="status">
				<Icon name="info" size={20} />The meeting is still running. The transcript appears here
				after it ends.
			</p>
		{:else if status.status === 'processing'}
			<p class="note" role="status">
				<Icon name="info" size={20} />
				<span>
					Transcribing, {minutes(status.progress.done)} of {minutes(status.progress.total)} minutes done.
					{#if status.retryAt !== undefined}
						The speech service has used up today’s allowance; this continues after midnight UTC, at
						{dayAndTime.format(status.retryAt)}.
					{/if}
				</span>
			</p>
		{:else if status.status === 'failed'}
			<p class="note alert" role="alert">
				<Icon name="alert" size={20} />The transcript could not be made.
			</p>
		{:else}
			{@const { transcript, audioUntil } = status}
			<p>
				{transcript.speakers.length > 0
					? `Speakers: ${transcript.speakers.map((s) => s.label).join(', ')}.`
					: 'Nobody spoke.'}
				{#if audioUntil > Date.now()}
					The recorded audio is kept until {dayAndTime.format(audioUntil)}, 48 hours after the
					meeting; the transcript stays until the host deletes it.
				{:else}
					The recorded audio was deleted 48 hours after the meeting; the transcript stays until the
					host deletes it.
				{/if}
			</p>
			<div class="actions">
				<a class="button quiet" href={transcriptFile(token, 'md')} download>
					<Icon name="download" size={20} />
					Download .md
				</a>
				<a class="button quiet" href={transcriptFile(token, 'json')} download>
					<Icon name="download" size={20} />
					Download .json
				</a>
				<CopyButton text={url} />
			</div>

			{#if transcript.paragraphs.length === 0}
				<p>Nothing was said, or nothing could be understood.</p>
			{:else}
				<ol class="text" aria-label="Transcript">
					{#each timeline(transcript) as entry, i (i)}
						{#if entry.kind === 'paragraph'}
							<li>
								<p>
									<strong>{entry.paragraph.label}</strong>
									<time datetime={new Date(entry.paragraph.startMs).toISOString()}
										>{clock.format(entry.paragraph.startMs)}</time
									>
								</p>
								<p>{entry.paragraph.text}</p>
							</li>
						{:else}
							<li class="gap">
								{clock.format(entry.gap.start)}: {duration(entry.gap.end - entry.gap.start)} of {entry.label}’s
								audio {entry.gap.reason === 'missing' ? 'is missing' : 'could not be transcribed'}.
							</li>
						{/if}
					{/each}
				</ol>
			{/if}
		{/if}

		{#if status.status !== 'ready'}
			<div class="actions">
				<CopyButton text={url} />
			</div>
		{/if}
	{/if}
</main>

<style>
	main {
		display: grid;
		align-content: start;
		gap: 1.5rem;
		min-height: 100dvh;
		max-width: 44rem;
		padding: var(--gutter);
		padding-top: calc(var(--gutter) + env(safe-area-inset-top));
		padding-bottom: calc(var(--gutter) * 2 + env(safe-area-inset-bottom));
	}

	.brand {
		color: var(--lichen);
		font-weight: 650;
		text-decoration: none;
	}

	header {
		display: grid;
		gap: 0.5rem;
	}

	h1 {
		font-size: var(--step-3);
		line-height: 1.1;
		font-weight: 750;
		letter-spacing: -0.02em;
		overflow-wrap: anywhere;
	}

	/* A plain card; the icon gives the tone (D34). */
	.note {
		display: flex;
		align-items: flex-start;
		gap: 0.75rem;
		padding: 0.75rem 1rem;
		border: 1px solid var(--reed);
		border-radius: var(--radius-s);
		background: var(--water);
	}

	.note :global(svg) {
		flex: none;
		margin-top: 0.125rem;
		color: var(--lichen);
	}

	.note.alert :global(svg) {
		color: var(--signal);
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}

	.actions :global(.button) {
		min-height: 44px;
		padding: 0 1.125rem;
	}

	.text {
		display: grid;
		gap: 1.25rem;
		margin: 0.5rem 0 0;
		padding: 0;
		list-style: none;
	}

	.text li {
		display: grid;
		gap: 0.25rem;
	}

	.text time {
		margin-left: 0.5rem;
		color: var(--lichen);
	}

	.text .gap {
		color: var(--lichen);
		font-style: italic;
	}
</style>
