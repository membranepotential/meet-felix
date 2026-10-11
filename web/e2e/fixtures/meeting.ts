/** Setting up rooms for protocol tests: joining as someone, and a live meeting with two people. */
import type { MeetingId, ParticipantId, RecordingPolicy } from '../../src/lib/protocol.ts';
import { createMeeting, type Api, type Auth, type OwnerSession } from './api.ts';
import type { RoomClient, ServerMsgOf } from './room-client.ts';
import type { Rooms } from './test.ts';

interface Joined {
	client: RoomClient;
	welcome: ServerMsgOf<'welcome'>;
	id: ParticipantId;
}

/** Opens a socket, sends `Join` with consent, and waits for the `Welcome`. */
export async function joinAs(
	rooms: Rooms,
	meeting: MeetingId,
	name: string,
	headers: Auth = {}
): Promise<Joined> {
	const client = await rooms.open(meeting, headers);
	client.join({ name });
	const welcome = await client.take('welcome');
	return { client, welcome, id: welcome.you };
}

interface LiveMeeting {
	meeting: MeetingId;
	host: Joined;
	guest: Joined;
}

/** A new meeting with the owner joined as host and a guest let in after them. */
export async function liveMeeting(
	{ rooms, api, owner }: { rooms: Rooms; api: Api; owner: OwnerSession },
	recording: RecordingPolicy = 'fromStart'
): Promise<LiveMeeting> {
	const meeting = await createMeeting(api, owner.headers, recording);
	const host = await joinAs(rooms, meeting, 'Host', owner.headers);
	const guest = await joinAs(rooms, meeting, 'Guest');
	return { meeting, host, guest };
}
