<script lang="ts">
	import { describeApiError, type ApiErrorWording } from '../api-messages.ts';
	import type { OwnerSession } from '../owner-session.svelte.ts';
	import Icon from './Icon.svelte';
	import PasswordField from './PasswordField.svelte';

	let {
		owner,
		onlogin
	}: {
		owner: OwnerSession;
		/** Runs after a successful login, once the page shows the owner's view. */
		onlogin?: () => void;
	} = $props();

	/** Typed or filled by the password manager each time; never stored (D24). */
	let password = $state('');
	let attempt = $derived(owner.state.kind === 'signed-out' ? owner.state.login : null);
	let sending = $derived(attempt?.kind === 'sending');

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		const typed = password.trim();
		if (typed === '' || sending) return;
		const ok = await owner.login(typed);
		password = '';
		if (ok) onlogin?.();
	}

	const WORDING: ApiErrorWording = {
		action: 'log you in',
		rateLimited: 'tries',
		unauthorized: 'That password is wrong.'
	};
</script>

<form onsubmit={submit}>
	<PasswordField bind:value={password} />
	{#if attempt?.kind === 'failed'}
		<p class="error" role="alert">
			<Icon name="alert" size={18} />{describeApiError(attempt.error, WORDING)}
		</p>
	{/if}
	<button class="button primary" type="submit" disabled={sending || password.trim() === ''}>
		{sending ? 'Logging in…' : 'Log in'}
	</button>
</form>

<style>
	form {
		display: grid;
		gap: 1.25rem;
	}
</style>
