/**
 * Client side of the Cloudflare Realtime SFU (placeholder; wired up in phase 2).
 *
 * Each participant holds two peer connections, so offers never collide ("glare"):
 * - publish: the client offers its tracks, the Room DO forwards the offer to the SFU and
 *   returns the answer (`ClientMsg::PublishOffer` → `ServerMsg::PublishAnswer`);
 * - subscribe: the Room DO offers remote tracks, the client answers
 *   (`ServerMsg::SubscribeOffer` → `ClientMsg::SubscribeAnswer`).
 *
 * The SFU is ICE-lite and returns its candidates in the SDP, so the client sends its offer
 * right after `setLocalDescription` without waiting for ICE gathering.
 */
import type { PublishedTrack, Rid, TrackSource } from './protocol.ts';

export interface SfuConfig {
	/** STUN/TURN servers with short-lived TURN credentials; the Room DO hands them out. */
	iceServers: RTCIceServer[];
}

export interface SfuSessions {
	publish: RTCPeerConnection;
	subscribe: RTCPeerConnection;
}

export function openSessions({ iceServers }: SfuConfig): SfuSessions {
	const config: RTCConfiguration = { iceServers, bundlePolicy: 'max-bundle' };
	return { publish: new RTCPeerConnection(config), subscribe: new RTCPeerConnection(config) };
}

export function closeSessions({ publish, subscribe }: SfuSessions): void {
	publish.close();
	subscribe.close();
}

/**
 * Camera simulcast: quarter, half and full resolution. The Room DO picks a layer per
 * receiver (`ClientMsg::SetLayer`) to match its tile size.
 */
export const CAMERA_SIMULCAST: readonly (RTCRtpEncodingParameters & { rid: Rid })[] = [
	{ rid: 'q', scaleResolutionDownBy: 4 },
	{ rid: 'h', scaleResolutionDownBy: 2 },
	{ rid: 'f', scaleResolutionDownBy: 1 }
];

export interface OutgoingTrack {
	track: MediaStreamTrack;
	source: TrackSource;
}

export interface PublishOffer {
	sdp: string;
	tracks: PublishedTrack[];
	/** Senders by source, for later `replaceSenderTrack` (device switch, iOS reacquire). */
	senders: Map<TrackSource, RTCRtpSender>;
}

/**
 * Adds `tracks` as send-only transceivers and creates the publish offer.
 * Camera video is simulcast in three layers; screen shares send one full layer, since
 * shrinking text makes it unreadable and the shared screen is always shown large.
 */
export async function createPublishOffer(
	pc: RTCPeerConnection,
	tracks: OutgoingTrack[]
): Promise<PublishOffer> {
	const added = tracks.map(({ track, source }) => {
		const transceiver = pc.addTransceiver(track, {
			direction: 'sendonly',
			...(source === 'camera' ? { sendEncodings: [...CAMERA_SIMULCAST] } : {})
		});
		if (track.kind === 'video') preferVideoCodecs(transceiver);
		return { transceiver, source, kind: track.kind === 'audio' ? 'audio' : 'video' } as const;
	});

	await pc.setLocalDescription(await pc.createOffer());
	const sdp = pc.localDescription?.sdp;
	if (!sdp) throw new Error('publish offer has no local description');

	const published: PublishedTrack[] = [];
	const senders = new Map<TrackSource, RTCRtpSender>();
	for (const { transceiver, source, kind } of added) {
		// `mid` is assigned by `setLocalDescription`.
		if (transceiver.mid === null) throw new Error(`no mid for ${source} transceiver`);
		published.push({ mid: transceiver.mid, kind, source });
		senders.set(source, transceiver.sender);
	}
	return { sdp, tracks: published, senders };
}

export async function applyPublishAnswer(pc: RTCPeerConnection, sdp: string): Promise<void> {
	await pc.setRemoteDescription({ type: 'answer', sdp });
}

/** Applies the Room DO's subscribe offer and returns the answer SDP to send back. */
export async function handleSubscribeOffer(pc: RTCPeerConnection, sdp: string): Promise<string> {
	await pc.setRemoteDescription({ type: 'offer', sdp });
	await pc.setLocalDescription(await pc.createAnswer());
	const answer = pc.localDescription?.sdp;
	if (!answer) throw new Error('subscribe answer has no local description');
	return answer;
}

/**
 * Reports each remote track with its `mid`; the room state maps `mid` to a participant.
 * Returns the unsubscribe.
 */
export function onRemoteTrack(
	pc: RTCPeerConnection,
	callback: (track: MediaStreamTrack, mid: string | null) => void
): () => void {
	const listener = (event: RTCTrackEvent) => callback(event.track, event.transceiver.mid);
	pc.addEventListener('track', listener);
	return () => pc.removeEventListener('track', listener);
}

/**
 * Pure: orders video codecs H.264 first, then VP8, and keeps the helper codecs
 * (RTX, RED, FEC). VP9 and AV1 are dropped: H.264 is hardware-encoded everywhere including
 * iOS, and AV1 is not smooth on iPhones (docs/media/README.md, D12).
 */
export function orderVideoCodecs(codecs: readonly RTCRtpCodec[]): RTCRtpCodec[] {
	const type = (c: RTCRtpCodec) => c.mimeType.toLowerCase();
	const h264 = codecs.filter((c) => type(c) === 'video/h264');
	const vp8 = codecs.filter((c) => type(c) === 'video/vp8');
	const helpers = codecs.filter((c) =>
		['video/rtx', 'video/red', 'video/ulpfec', 'video/flexfec-03'].includes(type(c))
	);
	return [...h264, ...vp8, ...helpers];
}

/**
 * Applies `orderVideoCodecs` where `setCodecPreferences` exists (Baseline 2024). Codecs come
 * from `RTCRtpReceiver.getCapabilities`, which is what browsers validate against.
 * Leaves the browser default in place if the API is missing or rejects the list.
 */
function preferVideoCodecs(transceiver: RTCRtpTransceiver): void {
	if (typeof transceiver.setCodecPreferences !== 'function') return;
	const codecs = RTCRtpReceiver.getCapabilities('video')?.codecs;
	if (!codecs) return;
	const ordered = orderVideoCodecs(codecs);
	if (!ordered.some((c) => ['video/h264', 'video/vp8'].includes(c.mimeType.toLowerCase()))) return;
	try {
		transceiver.setCodecPreferences(ordered);
	} catch {
		// InvalidAccessError / InvalidModificationError: keep the browser's order.
	}
}
