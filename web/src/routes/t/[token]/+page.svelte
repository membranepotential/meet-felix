<script lang="ts">
	import { page } from '$app/state';
	import CopyButton from '#lib/components/CopyButton.svelte';
	import Icon from '#lib/components/Icon.svelte';

	/**
	 * Placeholder until transcripts exist (phase 3). It already has the final page's parts:
	 * the meeting's facts, the recording, then the text as paragraphs, one per speaker turn,
	 * each with the time of day it was said.
	 */
	let url = $derived(page.url.href);
	const NOT_YET = 'Not available yet';
</script>

<svelte:head>
	<title>Transcript – meet-felix</title>
</svelte:head>

<main>
	<a class="brand" href="/">meet-felix</a>

	<header>
		<h1>Transcript</h1>
		<p class="status">
			Transcripts are not made yet. Once they are, this page shows the meeting’s recording and
			everything that was said.
		</p>
		<dl>
			<dt>Date</dt>
			<dd>{NOT_YET}</dd>
			<dt>Duration</dt>
			<dd>{NOT_YET}</dd>
			<dt>Participants</dt>
			<dd>{NOT_YET}</dd>
		</dl>
		<div class="actions">
			<button class="button quiet" type="button" disabled>
				<Icon name="download" size={20} />
				Download .md
			</button>
			<button class="button quiet" type="button" disabled>
				<Icon name="download" size={20} />
				Download .json
			</button>
			<CopyButton text={url} />
		</div>
	</header>

	<section aria-labelledby="recording-heading">
		<h2 id="recording-heading">Recording</h2>
		<p class="placeholder">Each speaker’s voice, to play or download.</p>
	</section>

	<section aria-labelledby="text-heading">
		<h2 id="text-heading">Text</h2>
		<p class="placeholder">
			One paragraph per speaker turn, with the speaker’s name and the time of day it was said.
		</p>
	</section>
</main>

<style>
	main {
		display: grid;
		align-content: start;
		gap: 2rem;
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

	header,
	section {
		display: grid;
		gap: 1rem;
	}

	h1 {
		font-size: var(--step-3);
		line-height: 1.1;
		font-weight: 750;
		letter-spacing: -0.02em;
	}

	h2 {
		font-size: var(--step-1);
		line-height: 1.25;
	}

	.status {
		padding: 0.75rem 1rem;
		border-radius: var(--radius-s);
		background: var(--water);
		box-shadow: inset 3px 0 0 var(--reed-strong);
	}

	dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.375rem 1.25rem;
		margin: 0;
	}

	dt {
		color: var(--lichen);
	}

	dd {
		margin: 0;
		color: var(--lichen);
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

	.placeholder {
		padding: 1rem;
		border-radius: var(--radius-s);
		border: 1px dashed var(--reed);
		color: var(--lichen);
	}
</style>
