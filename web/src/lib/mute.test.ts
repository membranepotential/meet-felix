import { describe, expect, it, vi } from 'vitest';
import { MuteReport } from './mute.ts';

/** A report whose socket is up while `up` is true; `sent` lists the flags that went out. */
function setup() {
	const sent: boolean[] = [];
	const socket = { up: true };
	const muteMic = vi.fn();
	const report = new MuteReport((muted) => socket.up && sent.push(muted) > 0, muteMic);
	return { report, sent, socket, muteMic };
}

describe('MuteReport', () => {
	it('reports switches, and one the socket missed on the next welcome', () => {
		const { report, sent, socket } = setup();
		report.welcomed(true, false, true);
		report.switched(false);
		socket.up = false;
		report.switched(true);
		socket.up = true;
		// The room still shows our last report, "muted": our unmute goes out.
		report.welcomed(false, true, true);
		expect(sent).toEqual([true, false]);
	});

	it('switches the mic off when a host muted us while the socket was down', () => {
		const { report, sent, muteMic } = setup();
		report.welcomed(true, false, true);
		report.welcomed(false, true, true);
		expect(muteMic).toHaveBeenCalledOnce();
		expect(sent).toEqual([]);
	});

	it('keeps the flag a welcome showed, so a later unmute while offline is not undone', () => {
		// The room had us unmuted. Outage 1: the user mutes, and a host mutes us too.
		const { report, sent, socket, muteMic } = setup();
		report.welcomed(true, false, true);
		report.welcomed(false, true, false);
		// Outage 2: the user unmutes.
		socket.up = false;
		report.switched(true);
		socket.up = true;
		report.welcomed(false, true, true);
		expect(muteMic).not.toHaveBeenCalled();
		expect(sent).toEqual([false]);
	});

	it('keeps the mute the room set at join, so a later unmute while offline is not undone', () => {
		// A host joins a room that records without their consent: muted before the welcome.
		const { report, sent, socket, muteMic } = setup();
		report.welcomed(true, true, false);
		socket.up = false;
		report.switched(true);
		socket.up = true;
		report.welcomed(false, true, true);
		expect(muteMic).not.toHaveBeenCalled();
		expect(sent).toEqual([false]);
	});

	it('forgets what the room held once we come back as a new participant', () => {
		const { report, sent, muteMic } = setup();
		report.welcomed(true, false, true);
		report.switched(false);
		// A new participant starts unmuted; the mic switched off is reported.
		report.welcomed(true, false, false);
		expect(sent).toEqual([true, true]);
		// Muted by the room at once and the mic still on: muting wins, as nothing is known.
		report.welcomed(true, true, true);
		expect(muteMic).toHaveBeenCalledOnce();
	});
});
