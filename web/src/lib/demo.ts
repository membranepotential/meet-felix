/**
 * Dev-only fixture: `/{meeting}?demo=N` fills the room with N participants (you included,
 * as host, with recording on) so the grid can be checked without a server.
 * Add `&speaker` to feature the first other participant as the active speaker.
 */
import type { MeetingId, Participant, ParticipantId, ServerMsg } from './protocol.ts';

const NAMES = ['Ada', 'Bruno', 'Chiara', 'Dmitri', 'Elif', 'Femi', 'Greta', 'Hiro', 'Inès'];

export function demoWelcome(meeting: MeetingId, myName: string, count: number): ServerMsg {
	const you = 'p0' as ParticipantId;
	const total = Math.min(10, Math.max(1, count));
	const participants: Participant[] = Array.from({ length: total }, (_, i) => ({
		id: `p${i}` as ParticipantId,
		name: i === 0 ? myName : (NAMES[(i - 1) % NAMES.length] ?? `Guest ${i}`),
		role: i === 0 ? 'Host' : 'Guest',
		tracks: []
	}));
	return {
		type: 'Welcome',
		you,
		state: {
			meeting,
			participants,
			locked: false,
			recording: { since: Date.now(), by: you }
		}
	};
}
