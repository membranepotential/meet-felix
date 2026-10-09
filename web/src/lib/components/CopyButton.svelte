<script lang="ts">
	import Icon from './Icon.svelte';

	let { text, label = 'Copy link' }: { text: string; label?: string } = $props();

	type CopyState = 'idle' | 'copied' | 'failed';
	let copyState = $state<CopyState>('idle');
	let resetTimer: ReturnType<typeof setTimeout> | undefined;

	async function copy() {
		try {
			await navigator.clipboard.writeText(text);
			copyState = 'copied';
		} catch {
			copyState = 'failed';
		}
		clearTimeout(resetTimer);
		resetTimer = setTimeout(() => (copyState = 'idle'), 2500);
	}
</script>

<button class="button quiet" type="button" onclick={copy}>
	<Icon name={copyState === 'copied' ? 'check' : 'copy'} size={20} />
	{copyState === 'copied' ? 'Copied' : label}
</button>
{#if copyState === 'failed'}
	<p class="failed" role="status">Copying failed. Select the link and copy it by hand.</p>
{/if}

<style>
	.failed {
		flex-basis: 100%;
		color: var(--lichen);
		font-size: var(--step--1);
	}
</style>
