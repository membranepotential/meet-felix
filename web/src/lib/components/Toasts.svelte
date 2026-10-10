<script lang="ts">
	import { flip } from 'svelte/animate';
	import { prefersReducedMotion } from 'svelte/motion';
	import { fade, fly } from 'svelte/transition';
	import type { Session } from '../session.svelte.ts';
	import type { ToastTone } from '../toasts.svelte.ts';
	import Icon from './Icon.svelte';

	let { session }: { session: Session } = $props();

	/** The connection problem, pinned while it lasts; `null` while connected. */
	let connection: { text: string; tone: ToastTone } | null = $derived.by(() => {
		const c = session.connection;
		if (c.kind === 'connecting')
			return { text: c.attempt > 1 ? 'Reconnecting…' : 'Connecting…', tone: 'info' };
		if (c.kind === 'waiting')
			return {
				text: `Connection lost. Retrying in ${Math.ceil(c.retryInMs / 1000)} s`,
				tone: 'alert'
			};
		return null;
	});

	/** The browser holds back the others' voices until a tap; the pill below is that tap. */
	const HEAR_OTHERS = 'Tap to hear the others';

	const NAMES = new Intl.ListFormat('en', { style: 'long', type: 'conjunction' });

	/** Who has not accepted the running recording, for the host; `null` while nobody. */
	let unconsented: string | null = $derived.by(() => {
		const people = session.awaitingConsent;
		if (people.length === 0) return null;
		const verb = people.length === 1 ? 'has' : 'have';
		return `${NAMES.format(people.map((p) => p.name))} ${verb} not accepted the recording yet`;
	});

	/** A notice for screen readers; `key` keeps each one in its live region while it lasts. */
	interface Announcement {
		key: string;
		text: string;
	}

	/**
	 * What the live regions below say, by tone. The connection note says it once: its countdown
	 * would be read out every second.
	 */
	let announced: Record<ToastTone, Announcement[]> = $derived.by(() => {
		const notes = [
			...session.toasts.items.map((t) => ({ key: `toast-${t.id}`, text: t.text, tone: t.tone })),
			...session.knocking.map((k) => ({
				key: `knock-${k.id}`,
				text: `${k.name} wants to join`,
				tone: 'info' as const
			})),
			...(unconsented ? [{ key: 'consent', text: unconsented, tone: 'info' as const }] : []),
			...(session.audio.blocked
				? [{ key: 'audio', text: HEAR_OTHERS, tone: 'alert' as const }]
				: []),
			...(connection
				? [
						{
							key: 'connection',
							text: connection.tone === 'alert' ? 'Connection lost. Retrying.' : connection.text,
							tone: connection.tone
						}
					]
				: [])
		];
		return {
			info: notes.filter((n) => n.tone === 'info'),
			alert: notes.filter((n) => n.tone === 'alert')
		};
	});

	/** Milliseconds for slides and fades; none when the user asks for reduced motion. */
	let motion = $derived(prefersReducedMotion.current ? 0 : 200);
</script>

{#snippet face(tone: ToastTone, text: string)}
	<span class={['icon', tone]}>
		<Icon name={tone === 'alert' ? 'alert' : 'info'} size={18} />
	</span>
	<span class="text">{text}</span>
{/snippet}

<!--
	Hangs above its positioned parent, the dock around the control bar, so a notice never moves
	the stage: centred on phones, in the stage's bottom-left corner on wide screens. Newest
	toasts sit lowest; knock requests, who has not accepted the recording, the "Tap to hear the
	others" button and the connection note stay put below them while they apply.
-->
<!--
	Screen readers announce text added to a live region that is already in the page, not one
	inserted together with its text. So both regions are always present, and the pills below
	carry no live role.
-->
<div class="visually-hidden" aria-live="polite">
	{#each announced.info as note (note.key)}
		<p>{note.text}</p>
	{/each}
</div>
<div class="visually-hidden" aria-live="assertive">
	{#each announced.alert as note (note.key)}
		<p>{note.text}</p>
	{/each}
</div>

<div class="stack">
	{#each session.toasts.items as toast (toast.id)}
		<div
			class="toast"
			animate:flip={{ duration: motion }}
			in:fly={{ y: 12, duration: motion }}
			out:fade={{ duration: motion }}
		>
			<button
				type="button"
				class="pill"
				title="Dismiss"
				onclick={() => session.toasts.dismiss(toast.id)}
			>
				{@render face(toast.tone, toast.text)}
			</button>
		</div>
	{/each}
	{#each session.knocking as knock (knock.id)}
		<div class="card" in:fly={{ y: 12, duration: motion }} out:fade={{ duration: motion }}>
			<p><strong>{knock.name}</strong> wants to join</p>
			<div class="knock-actions">
				<button
					class="button quiet"
					type="button"
					onclick={() => session.send({ type: 'kick', who: knock.id })}
				>
					Deny
				</button>
				<button
					class="button primary"
					type="button"
					onclick={() => session.send({ type: 'admit', who: knock.id })}
				>
					Let in
				</button>
			</div>
		</div>
	{/each}
	<!-- No button: only they can accept; it goes away when they do or recording stops. -->
	{#if unconsented}
		<p class="pill" transition:fade={{ duration: motion }}>
			{@render face('info', unconsented)}
		</p>
	{/if}
	{#if session.audio.blocked}
		<button
			type="button"
			class="pill"
			transition:fade={{ duration: motion }}
			onclick={() => session.audio.resume()}
		>
			{@render face('alert', HEAR_OTHERS)}
		</button>
	{/if}
	{#if connection}
		<p class="pill" transition:fade={{ duration: motion }}>
			{@render face(connection.tone, connection.text)}
		</p>
	{/if}
</div>

<style>
	.stack {
		position: absolute;
		inset-inline: 0;
		bottom: 100%;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.5rem;
		padding: 0 var(--gutter) 0.5rem;
		/* Taps between the notices reach the tiles underneath. */
		pointer-events: none;
	}

	.stack > * {
		max-width: min(28rem, 100%);
		pointer-events: auto;
	}

	/* Opaque, so the text stays readable over video. */
	.pill,
	.card {
		background: var(--water);
		color: var(--fog);
		box-shadow:
			inset 0 0 0 1px var(--reed),
			0 6px 20px rgb(4 10 12 / 0.45);
		font-size: var(--step--1);
		line-height: 1.35;
	}

	.pill {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		min-height: 44px;
		padding: 0.5rem 1rem 0.5rem 0.75rem;
		border: 0;
		border-radius: 22px;
		text-align: start;
	}

	button.pill {
		transition: background-color 120ms ease;
	}

	button.pill:hover {
		background: var(--reed);
	}

	.icon {
		display: inline-flex;
		flex: none;
		color: var(--lichen);
	}

	.icon.alert {
		color: var(--signal);
	}

	.text {
		overflow-wrap: anywhere;
	}

	.card {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: 0.5rem 1rem;
		width: min(28rem, 100%);
		padding: 0.625rem 0.625rem 0.625rem 1rem;
		border-radius: var(--radius-m);
	}

	.knock-actions {
		display: flex;
		gap: 0.5rem;
		margin-left: auto;
	}

	.knock-actions .button {
		min-height: 44px;
		padding: 0 1rem;
	}

	/* Wide screens: a narrower column in the stage's bottom-left corner, off the screen's middle. */
	@media (min-width: 761px) {
		.stack {
			align-items: flex-start;
		}

		.stack > * {
			max-width: min(22rem, 100%);
		}

		.card {
			width: min(22rem, 100%);
		}
	}
</style>
