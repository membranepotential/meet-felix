/** What a page receives over WebRTC, from `getStats()` of every peer connection it made. */
import type { Locator, Page } from '@playwright/test';

export interface Inbound {
	/** The stats id; stable for a stream while its connection lives. */
	id: string;
	kind: 'audio' | 'video';
	/** The id of the `MediaStreamTrack` the stream feeds. */
	trackIdentifier: string;
	bytesReceived: number;
	frameWidth: number | null;
	framesDecoded: number | null;
}

/** Every `inbound-rtp` stream of every open peer connection on `page`. */
export function inbound(page: Page): Promise<Inbound[]> {
	return page.evaluate(async () => {
		const connections = (window.__e2ePeerConnections ?? []).filter(
			(pc) => pc.connectionState !== 'closed'
		);
		const reports = await Promise.all(connections.map((pc) => pc.getStats()));
		return reports.flatMap((report) =>
			[...report.values()].flatMap((s: RTCInboundRtpStreamStats) =>
				s.type === 'inbound-rtp' && (s.kind === 'audio' || s.kind === 'video')
					? [
							{
								id: s.id,
								kind: s.kind,
								trackIdentifier: s.trackIdentifier,
								bytesReceived: s.bytesReceived ?? 0,
								frameWidth: s.frameWidth ?? null,
								framesDecoded: s.framesDecoded ?? null
							}
						]
					: []
			)
		);
	});
}

/** Streams whose `bytesReceived` grew between two samples. */
export function growing(before: readonly Inbound[], after: readonly Inbound[]): Inbound[] {
	const bytes = new Map(before.map((s) => [s.id, s.bytesReceived]));
	return after.filter((s) => s.bytesReceived > (bytes.get(s.id) ?? Infinity));
}

/** Samples twice, `ms` apart, and returns the streams that grew in between. */
export async function growingOver(page: Page, ms: number): Promise<Inbound[]> {
	const before = await inbound(page);
	await page.waitForTimeout(ms);
	return growing(before, await inbound(page));
}

/** The id of the video track a tile shows, or `null` while it shows initials. */
export function tileTrackId(tile: Locator): Promise<string | null> {
	return tile.evaluate((figure) => {
		const stream = figure.querySelector('video')?.srcObject;
		return stream instanceof MediaStream ? (stream.getVideoTracks()[0]?.id ?? null) : null;
	});
}

/** The decoded width of the video a tile shows, read from the element; 0 before a frame. */
export function tileVideoWidth(tile: Locator): Promise<number> {
	return tile.evaluate((figure) => figure.querySelector('video')?.videoWidth ?? 0);
}
