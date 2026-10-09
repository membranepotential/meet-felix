<script lang="ts" module>
	export type IconName =
		| 'mic'
		| 'mic-off'
		| 'camera'
		| 'camera-off'
		| 'screen'
		| 'leave'
		| 'lock'
		| 'unlock'
		| 'record'
		| 'end'
		| 'copy'
		| 'check'
		| 'host'
		| 'remove'
		| 'share'
		| 'download'
		| 'info'
		| 'alert'
		| 'fullscreen'
		| 'fullscreen-exit';
</script>

<script lang="ts">
	let { name, size = 24 }: { name: IconName; size?: number } = $props();
</script>

<svg
	width={size}
	height={size}
	viewBox="0 0 24 24"
	fill="none"
	stroke="currentColor"
	stroke-width="2"
	stroke-linecap="round"
	stroke-linejoin="round"
	aria-hidden="true"
>
	{#if name === 'mic' || name === 'mic-off'}
		<rect x="9" y="3" width="6" height="11" rx="3" />
		<path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
		{#if name === 'mic-off'}<path d="M3 3l18 18" />{/if}
	{:else if name === 'camera' || name === 'camera-off'}
		<rect x="3" y="6" width="13" height="12" rx="2" />
		<path d="M16 10l5-3v10l-5-3" />
		{#if name === 'camera-off'}<path d="M3 3l18 18" />{/if}
	{:else if name === 'screen'}
		<rect x="3" y="4" width="18" height="12" rx="2" />
		<path d="M8 20h8M12 16v4M12 13V7M9 10l3-3 3 3" />
	{:else if name === 'leave'}
		<path d="M4 15c4-4 12-4 16 0l-2 3-3-1v-3a10 10 0 0 0-6 0v3l-3 1z" />
	{:else if name === 'lock' || name === 'unlock'}
		<rect x="5" y="11" width="14" height="10" rx="2" />
		<path d={name === 'lock' ? 'M8 11V7a4 4 0 0 1 8 0v4' : 'M8 11V7a4 4 0 0 1 7.5-2'} />
	{:else if name === 'record'}
		<circle cx="12" cy="12" r="8" />
		<circle cx="12" cy="12" r="3.5" fill="currentColor" />
	{:else if name === 'end'}
		<rect x="5" y="5" width="14" height="14" rx="2" />
	{:else if name === 'copy'}
		<rect x="8" y="8" width="12" height="12" rx="2" />
		<path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
	{:else if name === 'check'}
		<path d="M5 12l5 5 9-10" />
	{:else if name === 'host'}
		<path d="M4 18h16M5 18l-1-10 5 4 3-6 3 6 5-4-1 10" />
	{:else if name === 'remove'}
		<circle cx="9" cy="8" r="4" />
		<path d="M2 20a7 7 0 0 1 14 0M16 11h6" />
	{:else if name === 'share'}
		<path d="M12 3v12M8 7l4-4 4 4M5 12v7a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-7" />
	{:else if name === 'download'}
		<path d="M12 4v11M8 11l4 4 4-4M5 20h14" />
	{:else if name === 'info'}
		<circle cx="12" cy="12" r="9" />
		<path d="M12 11v5M12 8h.01" />
	{:else if name === 'alert'}
		<path d="M10.3 4.6 2.6 18a2 2 0 0 0 1.7 3h15.4a2 2 0 0 0 1.7-3L13.7 4.6a2 2 0 0 0-3.4 0z" />
		<path d="M12 10v4M12 17h.01" />
	{:else if name === 'fullscreen'}
		<path d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5" />
	{:else if name === 'fullscreen-exit'}
		<path d="M9 4v5H4M20 9h-5V4M15 20v-5h5M4 15h5v5" />
	{/if}
</svg>
