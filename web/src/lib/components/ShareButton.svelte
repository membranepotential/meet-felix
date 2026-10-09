<script lang="ts">
	import { canShareLink, shareLink } from '../share.ts';
	import Icon from './Icon.svelte';

	/** Renders nothing where the browser has no share sheet. */
	let { url, title, primary = false }: { url: string; title: string; primary?: boolean } = $props();

	const available = canShareLink(navigator);
	let failed = $state(false);

	async function share() {
		failed = (await shareLink(url, title)) === 'failed';
	}
</script>

{#if available}
	<button class={['button', primary ? 'primary' : 'quiet']} type="button" onclick={share}>
		<Icon name="share" size={20} />
		Share…
	</button>
	{#if failed}
		<p class="failed" role="status">Sharing did not work. Copy the link instead.</p>
	{/if}
{/if}

<style>
	.failed {
		flex-basis: 100%;
		color: var(--lichen);
		font-size: var(--step--1);
	}
</style>
