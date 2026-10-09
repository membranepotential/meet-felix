<script lang="ts">
	/**
	 * The owner's password (the Worker's create secret, D7). Never stored by the app: the
	 * browser's password manager fills it each time.
	 */
	let { value = $bindable() }: { value: string } = $props();
</script>

<!-- Password managers file a password under a username; this fixed one keeps it findable. -->
<input
	class="visually-hidden"
	type="text"
	name="username"
	autocomplete="username"
	value="owner"
	readonly
	tabindex="-1"
	aria-hidden="true"
/>
<label class="field">
	<span>Password</span>
	<input
		type="password"
		name="password"
		bind:value
		autocomplete="current-password"
		enterkeyhint="go"
		required
	/>
</label>

<style>
	.field {
		display: grid;
		gap: 0.375rem;
		font-size: var(--step--1);
		color: var(--lichen);
	}

	input[type='password'] {
		min-height: var(--touch);
		padding: 0 1rem;
		border: 1px solid var(--reed);
		border-radius: var(--radius-s);
		background: var(--water);
		font-size: var(--step-0);
		letter-spacing: 0.08em;
		color: var(--fog);
	}

	input[type='password']:focus-visible {
		border-color: var(--lamp);
	}
</style>
