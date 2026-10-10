<script lang="ts">
	import type { LocalMedia } from '../local-media.svelte.ts';

	/**
	 * The microphone and camera selects, on the pre-join screen and in the meeting's devices
	 * dialog. Each select shows the device whose track is live, else the recorded choice.
	 */
	let { media }: { media: LocalMedia } = $props();

	let microphone = $derived(
		media.tracks.audio?.getSettings().deviceId ?? media.choice.microphone ?? ''
	);
	let camera = $derived(media.tracks.video?.getSettings().deviceId ?? media.choice.camera ?? '');

	// After a failed or overtaken switch the shown device is the one from before the pick, so
	// Svelte sees no change and would leave the pick in the select: set it back by hand.
	async function pickMicrophone(select: HTMLSelectElement) {
		await media.selectMicrophone(select.value);
		select.value = microphone;
	}

	async function pickCamera(select: HTMLSelectElement) {
		await media.selectCamera(select.value);
		select.value = camera;
	}
</script>

{#if media.devices.microphones.length > 0}
	<label class="field">
		<span>Microphone</span>
		<select value={microphone} onchange={(e) => pickMicrophone(e.currentTarget)}>
			{#each media.devices.microphones as device (device.id)}
				<option value={device.id}>{device.label}</option>
			{/each}
		</select>
	</label>
{/if}
{#if media.devices.cameras.length > 0}
	<label class="field">
		<span>Camera</span>
		<select value={camera} onchange={(e) => pickCamera(e.currentTarget)}>
			{#each media.devices.cameras as device (device.id)}
				<option value={device.id}>{device.label}</option>
			{/each}
		</select>
	</label>
{/if}

<style>
	.field {
		display: grid;
		gap: 0.375rem;
		font-size: var(--step--1);
		color: var(--lichen);
	}

	.field + .field {
		margin-top: 1rem;
	}

	select {
		min-height: 48px;
		padding: 0 0.875rem;
		border: 1px solid var(--reed);
		border-radius: var(--radius-s);
		background: var(--water);
		font-size: var(--step-0);
		color: var(--fog);
	}

	select:focus-visible {
		border-color: var(--lamp);
	}
</style>
