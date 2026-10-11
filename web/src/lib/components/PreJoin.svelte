<script lang="ts" module>
	import type { OpenPhase } from '../api.ts';

	/** The meeting's recording policy and phase, as far as the pre-join screen knows them. */
	export type PolicyLookup =
		| { kind: 'loading' }
		| { kind: 'failed' }
		| { kind: 'known'; policy: RecordingPolicy; phase: OpenPhase };
</script>

<script lang="ts">
	import { untrack } from 'svelte';
	import type { LocalMedia } from '../local-media.svelte.ts';
	import { describeCaptureError } from '../media/capture.ts';
	import { showStream } from '../media/video.ts';
	import type { OwnerSession } from '../owner-session.svelte.ts';
	import { hostOffer } from '../owner-state.ts';
	import type { MeetingId, RecordingPolicy } from '../protocol.ts';
	import DevicePickers from './DevicePickers.svelte';
	import Icon from './Icon.svelte';
	import HostLogin from './HostLogin.svelte';

	let {
		meeting,
		media,
		owner,
		lookup,
		name = $bindable(),
		onjoin,
		onretry
	}: {
		meeting: MeetingId;
		media: LocalMedia;
		/** A browser with the owner's session joins as host (D32). */
		owner: OwnerSession;
		lookup: PolicyLookup;
		name: string;
		/** `consent`: the user pressed "Join and accept recording". */
		onjoin: (consent: boolean) => void;
		onretry: () => void;
	} = $props();

	/** `DisplayName::MAX_CHARS` in the core. */
	const NAME_MAX = 40;
	let policy = $derived(lookup.kind === 'known' ? lookup.policy : null);
	let recorded = $derived(policy === 'fromStart');
	let canJoin = $derived(name.trim().length > 0 && policy !== null);
	/** The user opened "Join as host". */
	let hostFormOpen = $state(false);
	let offer = $derived(hostOffer(owner.state, hostFormOpen));
	let scheduled = $derived(lookup.kind === 'known' && lookup.phase === 'scheduled');

	function submit(event: SubmitEvent) {
		event.preventDefault();
		if (canJoin) onjoin(recorded);
	}

	/**
	 * The host is always the owner (D32), so an empty name field gets the owner's name, if the
	 * deployment sets one.
	 */
	function prefillOwner() {
		const ownerName = owner.state.kind === 'signed-in' ? owner.state.owner.name : null;
		if (ownerName !== null && name.trim() === '') name = ownerName;
	}

	// Once per login, not on every keystroke: the owner may still type another name.
	$effect(() => {
		if (owner.state.kind === 'signed-in') untrack(prefillOwner);
	});

	/** The login made this browser the host: continue the join with the owner's name. */
	function loggedIn() {
		prefillOwner();
		if (canJoin) onjoin(recorded);
	}
</script>

<main>
	<div class="preview">
		<video {@attach showStream(media.cameraStream)} class={[!media.camOn && 'hidden']}></video>
		{#if !media.camOn}
			<p class="camera-off">
				{media.starting || lookup.kind === 'loading' ? 'Starting camera…' : 'Camera is off'}
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

	<div class="side">
		<form onsubmit={submit}>
			<h1 class="meeting">{meeting}</h1>

			{#if media.error}
				<div class="error" role="alert">
					<Icon name="alert" size={18} />
					<p>{describeCaptureError(media.error)}</p>
					{#if media.error !== 'insecure'}
						<button class="button quiet" type="button" onclick={() => media.start()}
							>Try again</button
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

			{#if media.devices.microphones.length > 0 || media.devices.cameras.length > 0}
				<details class="devices">
					<summary>Microphone and camera</summary>
					<DevicePickers {media} />
				</details>
			{/if}

			{#if lookup.kind === 'failed'}
				<div class="error" role="alert">
					<Icon name="alert" size={18} />
					<p>The meeting could not be reached. Check your connection, then try again.</p>
					<button class="button quiet" type="button" onclick={onretry}>Try again</button>
				</div>
			{:else if policy === 'fromStart'}
				<div class="notice">
					<p class="notice-title">
						<Icon name="record" size={20} />
						This meeting is recorded
					</p>
					<p>
						Voices become a written transcript that everyone in the meeting gets. Video is not
						recorded.
					</p>
				</div>
			{:else if policy === 'hostDecides'}
				<p class="note">
					A host may start recording during the meeting. You will be asked before your voice is
					recorded.
				</p>
			{/if}

			{#if offer.kind === 'host'}
				<p class="as-host">You join as host</p>
			{:else if scheduled}
				<p class="note">The meeting has not started yet. You wait until the host arrives.</p>
			{/if}

			<div class="join-bar">
				<button class="button primary join" type="submit" disabled={!canJoin}>
					{lookup.kind === 'loading'
						? 'Checking the meeting…'
						: recorded
							? 'Join and accept recording'
							: 'Join'}
				</button>
				{#if offer.kind === 'link'}
					<button class="host-link" type="button" onclick={() => (hostFormOpen = true)}>
						Join as host
					</button>
				{/if}
			</div>
		</form>

		{#if offer.kind === 'form'}
			<HostLogin {owner} onlogin={loggedIn} />
		{/if}
	</div>
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

	/* Phones: one column; the join button stays at the bottom, in thumb reach. */
	@media (max-width: 760px) {
		main {
			grid-template-columns: 1fr;
			grid-template-rows: auto 1fr;
			align-items: stretch;
			gap: 1rem;
			padding-top: calc(var(--gutter) + env(safe-area-inset-top));
			padding-bottom: 0;
		}

		.join-bar {
			position: sticky;
			bottom: 0;
			margin-top: auto;
			padding: 0.75rem 0 calc(var(--gutter) + env(safe-area-inset-bottom));
			background: linear-gradient(to top, var(--lake) 75%, transparent);
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
			aspect-ratio: auto;
			height: min(42dvh, 120vw);
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
		background: var(--scrim-button);
		color: var(--fog);
		backdrop-filter: blur(6px);
	}

	.round.off {
		background: var(--fog);
		color: var(--lake);
	}

	.side {
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
	}

	/* Phones: the form fills the column, so its join bar sits at the bottom. */
	@media (max-width: 760px) {
		.side,
		form {
			flex: 1;
		}
	}

	form {
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
	}

	.as-host {
		color: var(--fog);
		font-weight: 650;
	}

	/* Quiet on purpose: only the owner needs it, and guests should not wonder about it. */
	.host-link {
		display: block;
		min-height: var(--touch);
		margin: 0.25rem auto 0;
		padding: 0 1rem;
		border: 0;
		background: none;
		color: var(--lichen);
		font-size: var(--step--1);
		text-decoration: underline;
		text-underline-offset: 0.2em;
		cursor: pointer;
	}

	.host-link:hover {
		color: var(--fog);
	}

	h1.meeting {
		color: var(--lichen);
		font-size: var(--step-0);
		font-weight: 650;
		line-height: 1.5;
	}

	.devices summary {
		padding: 0.625rem 0;
		color: var(--lichen);
		font-size: var(--step--1);
		cursor: pointer;
	}

	.devices[open] summary {
		margin-bottom: 0.5rem;
	}

	.field {
		display: grid;
		gap: 0.375rem;
		font-size: var(--step--1);
		color: var(--lichen);
	}

	input {
		min-height: 48px;
		padding: 0 0.875rem;
		border: 1px solid var(--reed);
		border-radius: var(--radius-s);
		background: var(--water);
		font-size: var(--step-0);
		color: var(--fog);
	}

	input:focus-visible {
		border-color: var(--lamp);
	}

	.notice {
		display: grid;
		gap: 0.375rem;
		padding: 1rem 1.125rem;
		border-radius: var(--radius-s);
		background: var(--signal-wash);
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

	.note {
		color: var(--lichen);
		font-size: var(--step--1);
		line-height: 1.5;
	}

	/* The shared card, with the retry button under the text. */
	.error {
		flex-wrap: wrap;
	}

	.error > p {
		flex: 1 1 12rem;
	}

	.error > .button {
		margin-left: calc(18px + 0.625rem);
	}

	.join {
		width: 100%;
		min-height: 64px;
		font-size: var(--step-1);
	}
</style>
