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
