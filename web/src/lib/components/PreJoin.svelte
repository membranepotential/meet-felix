<script lang="ts">
	import type { LocalMedia } from '../local-media.svelte.ts';
	import type { CaptureError } from '../media/capture.ts';
	import { showStream } from '../media/video.ts';
	import type { MeetingId } from '../protocol.ts';
	import Icon from './Icon.svelte';

	let {
		meeting,
		media,
		recorded,
		name = $bindable(),
		onjoin
	}: {
		meeting: MeetingId;
		media: LocalMedia;
		recorded: boolean;
		name: string;
		onjoin: () => void;
	} = $props();

	const NAME_MAX = 40;
	let canJoin = $derived(name.trim().length > 0);

	function submit(event: SubmitEvent) {
		event.preventDefault();
		if (canJoin) onjoin();
	}

	function describe(error: CaptureError): string {
		switch (error) {
			case 'denied':
				return 'Camera and microphone are blocked. Allow them in your browser’s site settings, then try again.';
			case 'not-found':
				return 'No camera or microphone was found. You can still join and listen.';
			case 'in-use':
				return 'Another app is using your camera or microphone. Close it, then try again.';
			case 'overconstrained':
				return 'The selected device cannot be used. Pick another one.';
			case 'insecure':
				return 'This page needs HTTPS to use your camera and microphone.';
			case 'unknown':
				return 'Camera and microphone could not start. Try again.';
		}
	}
</script>

<main>
	<div class="preview">
		<video {@attach showStream(media.cameraStream)} class={[!media.camOn && 'hidden']}></video>
		{#if !media.camOn}
			<p class="camera-off">
				{media.starting ? 'Starting camera…' : 'Camera is off'}
			</p>
		{/if}
		<div class="toggles">
			<button
				type="button"
				class={['round', !media.micOn && 'off']}
				aria-pressed={!media.micOn}
				onclick={() => media.toggleMic()}
				disabled={!media.tracks.audio}
			>
				<Icon name={media.micOn ? 'mic' : 'mic-off'} />
				<span class="visually-hidden">Mute microphone</span>
			</button>
			<button
				type="button"
				class={['round', !media.camOn && 'off']}
				aria-pressed={!media.camOn}
				onclick={() => media.toggleCamera()}
			>
				<Icon name={media.camOn ? 'camera' : 'camera-off'} />
				<span class="visually-hidden">Turn camera off</span>
			</button>
		</div>
	</div>

	<form onsubmit={submit}>
		<p class="meeting">{meeting}</p>
		<h1>Ready to join?</h1>

		{#if media.error}
			<div class="error" role="alert">
				<p>{describe(media.error)}</p>
				{#if media.error !== 'insecure'}
					<button class="button quiet" type="button" onclick={() => media.start()}>Try again</button
					>
				{/if}
			</div>
		{/if}

		<label class="field">
			<span>Your name</span>
			<input
				bind:value={name}
				maxlength={NAME_MAX}
				autocomplete="name"
				enterkeyhint="go"
				required
			/>
		</label>

		{#if media.devices.microphones.length > 0}
			<label class="field">
				<span>Microphone</span>
				<select
					value={media.tracks.audio?.getSettings().deviceId ?? media.choice.microphone ?? ''}
					onchange={(e) => media.selectMicrophone(e.currentTarget.value)}
				>
					{#each media.devices.microphones as device (device.id)}
						<option value={device.id}>{device.label}</option>
					{/each}
				</select>
			</label>
		{/if}

		{#if media.devices.cameras.length > 0}
			<label class="field">
				<span>Camera</span>
				<select
					value={media.tracks.video?.getSettings().deviceId ?? media.choice.camera ?? ''}
					onchange={(e) => media.selectCamera(e.currentTarget.value)}
				>
					{#each media.devices.cameras as device (device.id)}
						<option value={device.id}>{device.label}</option>
					{/each}
				</select>
			</label>
		{/if}

		{#if recorded}
			<div class="notice">
				<p class="notice-title">
					<Icon name="record" size={20} />
					This meeting is recorded
				</p>
				<p>
					Everyone’s voice is recorded and turned into a written transcript after the meeting.
					Everyone in the meeting gets the transcript link at the end. Video is not recorded.
				</p>
			</div>
		{/if}

		<button class="button primary join" type="submit" disabled={!canJoin}>
			{recorded ? 'Join and accept recording' : 'Join'}
		</button>
	</form>
</main>

<style>
	main {
		display: grid;
		grid-template-columns: minmax(0, 3fr) minmax(18rem, 2fr);
		align-items: center;
		gap: clamp(1.5rem, 4vw, 3.5rem);
		min-height: 100dvh;
		max-width: 72rem;
		margin: 0 auto;
		padding: var(--gutter);
	}

	@media (max-width: 760px) {
		main {
			grid-template-columns: 1fr;
			align-items: start;
		}
	}

	.preview {
		position: relative;
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-m);
		background: var(--water);
		overflow: hidden;
	}

	@media (max-width: 760px) and (orientation: portrait) {
		.preview {
			aspect-ratio: 4 / 5;
			max-height: 52dvh;
			width: 100%;
		}
	}

	video {
		width: 100%;
		height: 100%;
		object-fit: cover;
		/* A mirrored self-view matches what people expect from a mirror. */
		transform: scaleX(-1);
	}

	.hidden {
		visibility: hidden;
	}

	.camera-off {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		color: var(--lichen);
	}

	.toggles {
		position: absolute;
		inset: auto 0 1rem;
		display: flex;
		justify-content: center;
		gap: 0.75rem;
	}

	.round {
		display: grid;
		place-items: center;
		width: var(--touch);
		height: var(--touch);
		border: 0;
		border-radius: 50%;
		background: rgb(14 28 34 / 0.72);
		color: var(--fog);
		backdrop-filter: blur(6px);
	}

	.round.off {
		background: var(--fog);
		color: var(--lake);
	}

	form {
		display: grid;
		gap: 1.25rem;
	}

	.meeting {
		color: var(--lichen);
		font-weight: 650;
	}

	h1 {
		font-size: var(--step-2);
		line-height: 1.15;
		margin-top: -0.75rem;
	}

	.field {
		display: grid;
		gap: 0.375rem;
		font-size: var(--step--1);
		color: var(--lichen);
	}

	input,
	select {
		min-height: 48px;
		padding: 0 0.875rem;
		border: 1px solid var(--reed);
		border-radius: var(--radius-s);
		background: var(--water);
		font-size: var(--step-0);
		color: var(--fog);
	}

	input:focus-visible,
	select:focus-visible {
		border-color: var(--lamp);
	}

	.notice {
		display: grid;
		gap: 0.375rem;
		padding: 1rem 1.125rem;
		border-radius: var(--radius-s);
		background: rgb(229 72 77 / 0.1);
		box-shadow: inset 3px 0 0 var(--signal);
		font-size: var(--step--1);
		line-height: 1.5;
	}

	.notice-title {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		color: var(--fog);
		font-size: var(--step-0);
		font-weight: 650;
	}

	.notice-title :global(svg) {
		color: var(--signal);
	}

	.error {
		display: grid;
		gap: 0.75rem;
		justify-items: start;
		padding: 0.875rem 1rem;
		border-left: 3px solid var(--signal);
		background: var(--water);
	}

	.join {
		width: 100%;
	}
</style>
