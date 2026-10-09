/**
 * Pure state of the owner's session in this browser (D32): whether the session cookie is
 * valid, and the login or logout in flight. `owner-session.svelte.ts` runs the requests and
 * feeds their results through `reduce`.
 */
import type { ApiError, ApiResult } from './api.ts';

/** One login or logout request. */
export type Attempt = { kind: 'idle' } | { kind: 'sending' } | { kind: 'failed'; error: ApiError };

export type OwnerState =
	/** `GET /api/me` has not answered yet. */
	| { kind: 'checking' }
	/** No valid session: pages show the login form. A failed check shows its error there. */
	| { kind: 'signed-out'; login: Attempt }
	| { kind: 'signed-in'; logout: Attempt };

export type OwnerEvent =
	| { kind: 'checked'; result: ApiResult<boolean> }
	| { kind: 'login-sent' }
	| { kind: 'login-answered'; result: ApiResult<void> }
	| { kind: 'logout-sent' }
	| { kind: 'logout-answered'; result: ApiResult<void> }
	/** An owner request came back `unauthorized`: the session expired or was revoked. */
	| { kind: 'rejected' };

export const CHECKING: OwnerState = { kind: 'checking' };

const IDLE: Attempt = { kind: 'idle' };
const SENDING: Attempt = { kind: 'sending' };
const SIGNED_IN: OwnerState = { kind: 'signed-in', logout: IDLE };
const SIGNED_OUT: OwnerState = { kind: 'signed-out', login: IDLE };

/** Applies one event. Events that do not fit the current state change nothing. */
export function reduce(state: OwnerState, event: OwnerEvent): OwnerState {
	switch (event.kind) {
		case 'checked': {
			if (state.kind !== 'checking') return state;
			const { result } = event;
			if (!result.ok) return { kind: 'signed-out', login: { kind: 'failed', error: result.error } };
			return result.value ? SIGNED_IN : SIGNED_OUT;
		}
		case 'login-sent':
			return state.kind === 'signed-out' ? { kind: 'signed-out', login: SENDING } : state;
		case 'login-answered':
			if (state.kind !== 'signed-out' || state.login.kind !== 'sending') return state;
			return event.result.ok
				? SIGNED_IN
				: { kind: 'signed-out', login: { kind: 'failed', error: event.result.error } };
		case 'logout-sent':
			return state.kind === 'signed-in' ? { kind: 'signed-in', logout: SENDING } : state;
		case 'logout-answered': {
			if (state.kind !== 'signed-in' || state.logout.kind !== 'sending') return state;
			const { result } = event;
			// `unauthorized`: the session was already gone, which is what logging out wants.
			return result.ok || result.error.kind === 'unauthorized'
				? SIGNED_OUT
				: { kind: 'signed-in', logout: { kind: 'failed', error: result.error } };
		}
		case 'rejected':
			return SIGNED_OUT;
	}
}

/** What a meeting's pre-join or lobby offers about hosting. */
export type HostOffer =
	/** The session check is in flight: say nothing yet. */
	| { kind: 'none' }
	/** A valid session: this browser joins as host. */
	| { kind: 'host' }
	/** No session: a quiet "Join as host" link. */
	| { kind: 'link' }
	/** No session, and the link was opened: the password form. */
	| { kind: 'form' };

/**
 * The owner's display name, prefilled when the owner's session joins a meeting (D32: there is
 * exactly one owner, so the app can know their name).
 */
export const OWNER_NAME = 'Felix Peppert';

/**
 * Decides the hosting line of the pre-join and the lobby. `formOpen`: the user opened "Join
 * as host". Only that opens the form: guests never see a password field otherwise, not even
 * when the session check failed.
 */
export function hostOffer(state: OwnerState, formOpen: boolean): HostOffer {
	switch (state.kind) {
		case 'checking':
			return { kind: 'none' };
		case 'signed-in':
			return { kind: 'host' };
		case 'signed-out':
			return formOpen ? { kind: 'form' } : { kind: 'link' };
	}
}
