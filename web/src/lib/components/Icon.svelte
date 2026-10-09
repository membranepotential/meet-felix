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
		| 'remove';
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
	{/if}
</svg>
