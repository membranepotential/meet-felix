/**
 * Browser-local persistence. Every access is wrapped because storage throws in some
 * private modes and when site data is blocked; the app must work without it.
 */
import type { HostToken, MeetingId } from './protocol.ts';

const DISPLAY_NAME = 'meet-felix:name';
/** The Worker's create secret, entered once by the owner on the start page. */
const CREATE_SECRET = 'meet-felix:create-secret';
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

function remove(key: string): void {
	try {
		localStorage.removeItem(key);
	} catch {
		// Storage unavailable: nothing was stored.
	}
}

export const loadDisplayName = (): string => read(DISPLAY_NAME) ?? '';
export const saveDisplayName = (name: string): void => write(DISPLAY_NAME, name);

export const loadCreateSecret = (): string | null => read(CREATE_SECRET);
export const saveCreateSecret = (secret: string): void => write(CREATE_SECRET, secret);
export const forgetCreateSecret = (): void => remove(CREATE_SECRET);

/**
 * Meeting words are recycled, so a stored token may belong to an earlier meeting with the
 * same words; the session forgets it when the room rejects it.
 */
export const loadHostToken = (meeting: MeetingId): HostToken | null => read(hostTokenKey(meeting));
export const saveHostToken = (meeting: MeetingId, token: HostToken): void =>
	write(hostTokenKey(meeting), token);
export const forgetHostToken = (meeting: MeetingId): void => remove(hostTokenKey(meeting));
