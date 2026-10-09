<script lang="ts">
	import type { ApiError } from '../api.ts';
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

	function describe(error: ApiError): string {
		switch (error.kind) {
			case 'unauthorized':
				return 'That password is wrong.';
			case 'rate-limited':
				return 'Too many tries in a short time. Wait a minute, then try again.';
			case 'network':
				return 'The server could not be reached. Check your connection, then try again.';
			case 'server':
				return error.status === 404
					? 'This server cannot log in yet. Update the Worker, then try again.'
					: `The server could not log you in (error ${error.status}). Try again.`;
			case 'malformed':
				return 'The server sent a reply this page does not understand. Try again.';
		}
	}
</script>

<form onsubmit={submit}>
	<PasswordField bind:value={password} />
	{#if attempt?.kind === 'failed'}
		<p class="error" role="alert"><Icon name="alert" size={18} />{describe(attempt.error)}</p>
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
