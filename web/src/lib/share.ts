/** The system share sheet (Web Share API), offered only where the browser has it. */

/** Whether "Share…" can be offered: `navigator.share` exists (mostly phones and Safari). */
export const canShareLink = (nav: { share?: unknown } | undefined): boolean =>
	typeof nav?.share === 'function';

export type ShareOutcome = 'shared' | 'cancelled' | 'failed';

/** Opens the share sheet for `url`. Closing the sheet counts as cancelled, not failed. */
export async function shareLink(url: string, title: string): Promise<ShareOutcome> {
	try {
		await navigator.share({ title, url });
		return 'shared';
	} catch (e) {
		return e instanceof DOMException && e.name === 'AbortError' ? 'cancelled' : 'failed';
	}
}
