<script lang="ts">
	import type { OwnerSession } from '../owner-session.svelte.ts';
	import LoginForm from './LoginForm.svelte';

	/** The owner's password form, where a guest's pre-join or lobby offers "Join as host". */
	let { owner, onlogin }: { owner: OwnerSession; onlogin: () => void } = $props();
</script>

<section class="host-login" aria-labelledby="host-heading">
	<h2 id="host-heading">Join as host</h2>
	<LoginForm {owner} {onlogin} />
</section>

<style>
	.host-login {
		display: grid;
		gap: 1rem;
		max-width: 26rem;
		padding: 1rem;
		border-radius: var(--radius-s);
		background: var(--water);
		box-shadow: inset 0 0 0 1px var(--reed);
	}

	h2 {
		font-size: var(--step-0);
		font-weight: 650;
	}
</style>
