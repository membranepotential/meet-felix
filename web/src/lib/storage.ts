/**
 * Browser-local persistence. Every access is wrapped because storage throws in some
 * private modes and when site data is blocked; the app must work without it.
 */
import type { MeetingId } from './protocol.ts';

const DISPLAY_NAME = 'meet-felix:name';
const hostTokenKey = (meeting: MeetingId) => `meet-felix:host:${meeting}`;

function read(key: string): string | null {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
}

function write(key: string, value: string): void {
	try {
		localStorage.setItem(key, value);
	} catch {
		// Storage unavailable: the value lives only for this page load.
	}
}

export const loadDisplayName = (): string => read(DISPLAY_NAME) ?? '';
export const saveDisplayName = (name: string): void => write(DISPLAY_NAME, name);

export const loadHostToken = (meeting: MeetingId): string | null => read(hostTokenKey(meeting));
export const saveHostToken = (meeting: MeetingId, token: string): void =>
	write(hostTokenKey(meeting), token);
