/**
 * Keeps the screen on during a meeting where the Screen Wake Lock API exists
 * (Chromium, Safari/iOS from 18.4). Browsers drop the lock when the page is hidden,
 * so it is requested again when the page becomes visible. Never throws.
 *
 * Returns a function that releases the lock for good.
 */
export function holdWakeLock(): () => void {
	if (typeof navigator === 'undefined' || !('wakeLock' in navigator)) return () => {};

	let sentinel: WakeLockSentinel | null = null;
	let released = false;

	async function request(): Promise<void> {
		if (released || document.visibilityState !== 'visible') return;
		if (sentinel !== null && !sentinel.released) return;
		try {
			const next = await navigator.wakeLock.request('screen');
			if (released) await next.release();
			else sentinel = next;
		} catch {
			// Denied (battery saver, permissions policy) or unsupported: the screen may dim.
		}
	}

	const onVisibility = () => void request();
	document.addEventListener('visibilitychange', onVisibility);
	void request();

	return () => {
		released = true;
		document.removeEventListener('visibilitychange', onVisibility);
		sentinel?.release().catch(() => {});
		sentinel = null;
	};
}
