/**
 * Binds a `MediaStream` to a `<video>` element. Usable as a Svelte attachment
 * (`{@attach showStream(stream)}`) or called directly; it returns its cleanup.
 *
 * iOS Safari needs `playsinline` or it opens video full screen, and it only autoplays
 * muted video without a user gesture. Local previews are always muted to avoid echo;
 * remote audio plays through separate `<audio>` elements.
 */
export function showStream(stream: MediaStream | null) {
	return (video: HTMLVideoElement): (() => void) => {
		video.playsInline = true;
		video.setAttribute('playsinline', '');
		video.muted = true;
		video.autoplay = true;
		video.srcObject = stream;
		if (stream) video.play().catch(() => {});
		return () => {
			video.srcObject = null;
		};
	};
}

/**
 * Plays a remote participant's audio. Usable as a Svelte attachment on `<audio>`. Browsers
 * allow it because the user pressed Join, which counts as a gesture on the page.
 */
export function playAudio(stream: MediaStream) {
	return (audio: HTMLAudioElement): (() => void) => {
		audio.autoplay = true;
		audio.srcObject = stream;
		audio.play().catch(() => {});
		return () => {
			audio.srcObject = null;
		};
	};
}

/** Reports an element's rendered width in CSS pixels whenever it changes. */
export function observeWidth(onWidth: (width: number) => void) {
	return (element: HTMLElement): (() => void) => {
		const observer = new ResizeObserver(([entry]) => {
			if (entry) onWidth(entry.contentRect.width);
		});
		observer.observe(element);
		return () => observer.disconnect();
	};
}
