/**
 * The media side of one stay: the two SFU peer connections (D10 in docs/media), replaced as the
 * room replaces their sessions (D33), the ICE servers they use, the layer requests and the
 * received tracks. The session hands it the room's media commands; it sends its own messages to
 * the room and reads nothing of the room's state.
 */
import { getIceServers } from './api.ts';
import type { MediaCommand } from './client-state.ts';
import { GraceNotice } from './grace.ts';
import { LayerControl } from './layers.ts';
import { IceServers } from './media/ice.ts';
import type {
	ClientMsg,
	MeetingId,
	SessionEpoch,
	SessionEpochs,
	SessionKind,
	TrackKind,
	TrackRef
} from './protocol.ts';
import { SessionResets } from './resets.ts';
import {
	NO_REMOTE_TRACKS,
	peerConnection,
	Publisher,
	Subscriber,
	type Outgoing,
	type PublishFailure,
	type RemoteTracks
} from './sfu.ts';
import type { ToastTone } from './toasts.svelte.ts';

/** How long a failed subscribe may take to recover before the user hears about it. */
const SUBSCRIBE_GRACE_MS = 10_000;

const KIND_LABEL: Record<TrackKind, string> = {
	audio: 'microphone',
	video: 'camera',
	screen: 'screen'
};

/** The quiet notice while the room replaces a session whose connection failed. */
const RECONNECTING: Record<SessionKind, string> = {
	publish: 'Reconnecting your camera and microphone…',
	subscribe: 'Reconnecting to the others…'
};

export class MediaLink {
	/** Sends `SetLayer` as tiles change size. */
	readonly layers: LayerControl;
	readonly #send: (msg: ClientMsg) => boolean;
	readonly #outgoing: () => Outgoing;
	readonly #notify: (text: string, tone: ToastTone) => void;
	readonly #ice: IceServers;
	readonly #resets: SessionResets;
	/** The first pull often races the publisher's connection and succeeds on the room's retry. */
	readonly #subscribeNotice: GraceNotice;
	/** The two peer connections; `null` until the first welcome. */
	#publisher: Publisher | null = null;
	#subscriber: Subscriber | null = null;
	/** The epochs of the room's sessions the two connections belong to. */
	#epochs: SessionEpochs = { publish: 0, subscribe: 0 };
	#remote: RemoteTracks = $state.raw(NO_REMOTE_TRACKS);

	/**
	 * Fetches the ICE servers at once. `send` sends a message to the room and returns `false` if
	 * the socket is down; `outgoing` gives the local tracks to publish; `notify` shows a notice.
	 */
	constructor(
		meeting: MeetingId,
		send: (msg: ClientMsg) => boolean,
		outgoing: () => Outgoing,
		notify: (text: string, tone: ToastTone) => void
	) {
		this.#send = send;
		this.#outgoing = outgoing;
		this.#notify = notify;
		this.layers = new LayerControl((track, rid) => send({ type: 'setLayer', track, rid }));
		this.#resets = new SessionResets((kind, epoch) => send({ type: 'resetSession', kind, epoch }));
		this.#subscribeNotice = new GraceNotice(SUBSCRIBE_GRACE_MS, (text) => notify(text, 'alert'));
		// Fetched while the socket connects, and again before the TURN credentials expire; the
		// open peer connections get each new set.
		this.#ice = new IceServers(
			() => getIceServers(meeting),
			(servers) => {
				this.#publisher?.setIceServers(servers);
				this.#subscriber?.setIceServers(servers);
			}
		);
	}

	/** The tracks the current subscribe connection received, by `trackKey`. */
	get remote(): RemoteTracks {
		return this.#remote;
	}

	run(command: MediaCommand): void {
		switch (command.kind) {
			case 'welcomed':
				this.#welcomed(command.fresh, command.epochs);
				return;
			case 'publish-answer':
				this.#publisher?.answer(command.sdp, command.rejected);
				return;
			case 'publish-rejected':
				this.#publisher?.rejected();
				return;
			case 'publish-reset':
				this.#openPublisher(command.epoch);
				return;
			case 'subscribe-offer':
				this.#subscribeNotice.cancel();
				this.#subscriber?.offer(command.sdp, command.tracks);
				return;
			case 'subscribe-reset':
				this.#openSubscriber(command.epoch);
				return;
			case 'subscribe-retrying':
				this.#subscribeNotice.start(command.notice);
				return;
			case 'layer-refused':
				this.layers.notSubscribed();
				return;
		}
	}

	/** The local tracks changed: device switches, camera on/off, screen share. */
	publish(outgoing: Outgoing): void {
		this.#publisher?.sync(outgoing);
	}

	/** The room lists `published` as the tracks that exist: drop the received ones it no longer does. */
	retain(published: readonly TrackRef[]): void {
		this.#subscriber?.retain(published);
	}

	dispose(): void {
		this.#ice.dispose();
		this.#closePublisher();
		this.#closeSubscriber();
		this.#resets.cancel();
		this.#subscribeNotice.cancel();
		this.layers.dispose();
	}

	/**
	 * A new participant opens new peer connections. After a reconnect, the old ones stay,
	 * except a connection whose session the room replaced while the socket was down: its reset
	 * never arrived, but the welcome's epochs tell.
	 */
	#welcomed(fresh: boolean, epochs: SessionEpochs): void {
		this.#ice.recover();
		const current = (kind: SessionKind) => !fresh && this.#epochs[kind] === epochs[kind];
		if (this.#publisher === null || !current('publish')) this.#openPublisher(epochs.publish);
		else this.#publisher.retryFailed();
		if (this.#subscriber === null || !current('subscribe')) this.#openSubscriber(epochs.subscribe);
	}

	/** The room's publish session of `epoch`: a new peer connection that publishes it all again. */
	#openPublisher(epoch: SessionEpoch): void {
		this.#closePublisher();
		this.#epochs = { ...this.#epochs, publish: epoch };
		this.#publisher = new Publisher(
			this.#ice.latest.then(peerConnection),
			this.#send,
			(kinds, reason) => this.#onPublishFailed(kinds, reason),
			() => this.#onConnectionFailed('publish', epoch)
		);
		this.#publisher.sync(this.#outgoing());
	}

	/** The room's subscribe session of `epoch`: a new peer connection that receives nothing yet. */
	#openSubscriber(epoch: SessionEpoch): void {
		this.#closeSubscriber();
		this.#epochs = { ...this.#epochs, subscribe: epoch };
		this.#subscriber = new Subscriber(
			this.#ice.latest.then(peerConnection),
			(sdp) => this.#send({ type: 'subscribeAnswer', sdp }),
			(update) => (this.#remote = update(this.#remote)),
			(delivered) => this.layers.received(delivered),
			() => this.#onConnectionFailed('subscribe', epoch)
		);
	}

	#closePublisher(): void {
		this.#publisher?.close();
		this.#publisher = null;
		this.#resets.done('publish');
	}

	#closeSubscriber(): void {
		this.#subscriber?.close();
		this.#subscriber = null;
		this.#resets.done('subscribe');
		this.#remote = NO_REMOTE_TRACKS;
		this.layers.reset();
	}

	/** The peer connection for a session failed: ask the room for a new one, quietly. */
	#onConnectionFailed(kind: SessionKind, epoch: SessionEpoch): void {
		if (this.#resets.failed(kind, epoch)) this.#notify(RECONNECTING[kind], 'info');
	}

	#onPublishFailed(kinds: TrackKind[], reason: PublishFailure): void {
		// The room's rejection already produced a notice from its error code.
		if (reason === 'rejected') return;
		const what = kinds.map((k) => KIND_LABEL[k]).join(' and ');
		this.#notify(
			reason === 'not-sent'
				? `Your ${what} is shared once the connection is back.`
				: `Your ${what} could not be shared. Turn it off and on to try again.`,
			'alert'
		);
	}
}
