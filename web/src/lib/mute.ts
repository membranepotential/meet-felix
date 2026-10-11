/**
 * The microphone's mute flag in the room: reports the user's switches, and after each welcome
 * makes the switch and the room's flag agree (`syncMute` in `client-state.ts`).
 */
import { syncMute } from './client-state.ts';

export class MuteReport {
	/** Sends `SetMuted`; returns `false` if the socket is down. */
	readonly #send: (muted: boolean) => boolean;
	readonly #muteMic: () => void;
	/**
	 * The flag the room holds for us, as far as this client knows: the last one that got
	 * through, or the one the last welcome showed. `null` for a new participant until then.
	 */
	#known: boolean | null = null;

	constructor(send: (muted: boolean) => boolean, muteMic: () => void) {
		this.#send = send;
		this.#muteMic = muteMic;
	}

	/** The user switched the mic while in the meeting. A report the socket missed waits for the next welcome. */
	switched(micOn: boolean): void {
		this.#report(!micOn);
	}

	/** A welcome showed the room's flag; `fresh` means a new participant. */
	welcomed(fresh: boolean, roomMuted: boolean, micOn: boolean): void {
		if (fresh) this.#known = null;
		switch (syncMute(roomMuted, micOn, this.#known)) {
			case 'none':
				this.#known = roomMuted;
				return;
			case 'mute-mic':
				this.#known = roomMuted;
				this.#muteMic();
				return;
			case 'report':
				this.#report(!micOn);
				return;
		}
	}

	#report(muted: boolean): void {
		if (this.#send(muted)) this.#known = muted;
	}
}
