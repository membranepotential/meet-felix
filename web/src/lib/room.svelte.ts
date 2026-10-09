/**
 * Reactive mirror of the Room DO's `RoomState`. The server owns the state: this class
 * changes only when a `Welcome` or `State` message arrives, never from local actions.
 */
import type { Participant, ParticipantId, RoomState, ServerMsg } from './protocol.ts';

export type StateMsg = Extract<ServerMsg, { type: 'Welcome' | 'State' }>;

export class Room {
	/** Replaced wholesale on every broadcast, so it is not deeply proxied. */
	state = $state.raw<RoomState | null>(null);
	you = $state.raw<ParticipantId | null>(null);

	me: Participant | null = $derived(
		this.state?.participants.find((p) => p.id === this.you) ?? null
	);
	others: Participant[] = $derived(this.state?.participants.filter((p) => p.id !== this.you) ?? []);
	isHost = $derived(this.me?.role === 'Host');
	recording = $derived(this.state?.recording ?? null);
	locked = $derived(this.state?.locked ?? false);

	apply(msg: StateMsg): void {
		if (msg.type === 'Welcome') this.you = msg.you;
		this.state = msg.state;
	}
}
