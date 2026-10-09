/**
 * Just enough SDP for the mock SFU: it parses an endpoint's offer and writes an answer that
 * Chromium and Firefox accept, and it writes the offers that carry pulled tracks. No media
 * ever flows (the answers and offers carry no ICE candidates), but the browsers' peer
 * connections negotiate as they do against the real SFU, so publishing, unpublishing and
 * subscribing follow the same code paths. Pure: randomness comes in as arguments.
 */

/** One `m=` section: its media line and its `a=` attributes, without the `a=` prefix. */
export interface MediaSection {
	media: string;
	port: number;
	proto: string;
	formats: string[];
	attributes: string[];
}

export interface ParsedSdp {
	/** Session-level attributes, without the `a=` prefix. */
	attributes: string[];
	media: MediaSection[];
}

/** What a peer says about its end of the connection; fixed for the life of a session. */
export interface Transport {
	ufrag: string;
	pwd: string;
	/** `sha-256` fingerprint, colon-separated hex. */
	fingerprint: string;
}

/** The `o=` line's session id and version; the version grows with every description. */
export interface Origin {
	id: string;
	version: number;
}

export type Direction = 'sendrecv' | 'sendonly' | 'recvonly' | 'inactive';

/** A media kind on the wire: a screen share is video too. */
export type SdpMediaKind = 'audio' | 'video';

/** A track in an offer the SFU (or a fake client) sends. */
export interface OfferedTrack {
	mid: string;
	kind: SdpMediaKind;
	direction: Extract<Direction, 'sendonly' | 'inactive'>;
	ssrc: number;
	/** `a=msid` stream and track ids. */
	stream: string;
	track: string;
}

const DIRECTIONS: readonly Direction[] = ['sendrecv', 'sendonly', 'recvonly', 'inactive'];
const isDirection = (value: string): value is Direction =>
	(DIRECTIONS as readonly string[]).includes(value);

/** Parses `sdp`; `null` if it is not an SDP document. */
export function parseSdp(sdp: string): ParsedSdp | null {
	const lines = sdp.split(/\r?\n/).filter((line) => line !== '');
	if (lines[0] !== 'v=0') return null;
	const parsed: ParsedSdp = { attributes: [], media: [] };
	for (const line of lines) {
		if (line.startsWith('m=')) {
			const [media, port, proto, ...formats] = line.slice(2).split(' ');
			if (media === undefined || port === undefined || proto === undefined) return null;
			parsed.media.push({ media, port: Number(port), proto, formats, attributes: [] });
		} else if (line.startsWith('a=')) {
			(parsed.media.at(-1)?.attributes ?? parsed.attributes).push(line.slice(2));
		}
	}
	return parsed;
}

/** The value of the first `a=name:value` attribute, or `undefined`. */
export function attribute(attributes: readonly string[], name: string): string | undefined {
	const prefix = `${name}:`;
	return attributes.find((a) => a.startsWith(prefix))?.slice(prefix.length);
}

export function direction(section: MediaSection): Direction {
	return section.attributes.find(isDirection) ?? 'sendrecv';
}

/** A fresh transport from `random` bytes: 4 for the ufrag, 18 for the password, 32 for the fingerprint. */
export function newTransport(random: (bytes: number) => Uint8Array): Transport {
	const hex = (bytes: Uint8Array) => [...bytes].map((b) => b.toString(16).padStart(2, '0'));
	return {
		ufrag: hex(random(4)).join(''),
		pwd: hex(random(18)).join(''),
		fingerprint: hex(random(32)).join(':').toUpperCase()
	};
}

function transportLines(transport: Transport, setup: 'actpass' | 'passive'): string[] {
	return [
		`a=ice-ufrag:${transport.ufrag}`,
		`a=ice-pwd:${transport.pwd}`,
		`a=fingerprint:sha-256 ${transport.fingerprint}`,
		`a=setup:${setup}`
	];
}

function sessionLines(origin: Origin, bundle: readonly string[]): string[] {
	return [
		'v=0',
		`o=- ${origin.id} ${origin.version} IN IP4 127.0.0.1`,
		's=-',
		't=0 0',
		...(bundle.length > 0 ? [`a=group:BUNDLE ${bundle.join(' ')}`] : []),
		'a=ice-lite',
		'a=msid-semantic: WMS *'
	];
}

const document = (lines: readonly string[]): string => `${lines.join('\r\n')}\r\n`;

/** Codec, header-extension and feedback attributes an answer may echo from the offer. */
const ECHOED = ['rtpmap:', 'fmtp:', 'rtcp-fb:', 'extmap:', 'rtcp-rsize'];

const ANSWER_DIRECTION: Record<Direction, Direction> = {
	sendrecv: 'recvonly',
	sendonly: 'recvonly',
	// The SFU never sends on a session that publishes.
	recvonly: 'inactive',
	inactive: 'inactive'
};

/**
 * Accepts every RTP section of `offer` as a receiver, with all offered codecs and header
 * extensions and the offer's simulcast layers; rejects stopped and non-RTP sections.
 */
export function answerSdp(offer: ParsedSdp, transport: Transport, origin: Origin): string {
	const bundle: string[] = [];
	const media = offer.media.flatMap((section): string[] => {
		const mid = attribute(section.attributes, 'mid');
		// Port 0 marks a stopped section, except with `bundle-only`: Firefox offers a new
		// transceiver that way when it reuses a stopped one's place.
		const rejected =
			(section.port === 0 && !section.attributes.includes('bundle-only')) ||
			section.media === 'application' ||
			mid === undefined;
		const head = [
			`m=${section.media} ${rejected ? 0 : 9} ${section.proto} ${section.formats.join(' ')}`,
			'c=IN IP4 0.0.0.0'
		];
		if (rejected) return [...head, ...(mid === undefined ? [] : [`a=mid:${mid}`]), 'a=inactive'];
		bundle.push(mid);
		const rids = section.attributes
			.filter((a) => a.startsWith('rid:'))
			.map((a) => a.slice('rid:'.length).split(' ')[0] ?? '');
		const simulcast = attribute(section.attributes, 'simulcast')?.replace(/^send /, '');
		return [
			...head,
			'a=rtcp:9 IN IP4 0.0.0.0',
			...transportLines(transport, 'passive'),
			`a=mid:${mid}`,
			`a=${ANSWER_DIRECTION[direction(section)]}`,
			'a=rtcp-mux',
			...section.attributes.filter((a) => ECHOED.some((p) => a.startsWith(p))).map((a) => `a=${a}`),
			...rids.map((rid) => `a=rid:${rid} recv`),
			...(simulcast === undefined ? [] : [`a=simulcast:recv ${simulcast}`])
		];
	});
	return document([...sessionLines(origin, bundle), ...media]);
}

const CODECS: Record<SdpMediaKind, { formats: string; lines: string[] }> = {
	audio: {
		formats: '111',
		lines: ['a=rtpmap:111 opus/48000/2', 'a=fmtp:111 minptime=10;useinbandfec=1']
	},
	video: {
		formats: '96',
		lines: [
			'a=rtpmap:96 VP8/90000',
			'a=rtcp-fb:96 nack',
			'a=rtcp-fb:96 nack pli',
			'a=rtcp-fb:96 ccm fir'
		]
	}
};

/** An offer that sends `tracks`, in order: every earlier section stays, as SDP requires. */
export function offerSdp(
	tracks: readonly OfferedTrack[],
	transport: Transport,
	origin: Origin
): string {
	const media = tracks.flatMap((t) => [
		`m=${t.kind} 9 UDP/TLS/RTP/SAVPF ${CODECS[t.kind].formats}`,
		'c=IN IP4 0.0.0.0',
		'a=rtcp:9 IN IP4 0.0.0.0',
		...transportLines(transport, 'actpass'),
		`a=mid:${t.mid}`,
		`a=${t.direction}`,
		`a=msid:${t.stream} ${t.track}`,
		'a=rtcp-mux',
		...CODECS[t.kind].lines,
		`a=ssrc:${t.ssrc} cname:${t.stream}`
	]);
	return document([
		...sessionLines(
			origin,
			tracks.map((t) => t.mid)
		),
		...media
	]);
}
