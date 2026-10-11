import { describe, expect, it } from 'vitest';
import type { ApiResult, Owner } from './api.ts';
import { CHECKING, hostOffer, reduce, type OwnerEvent, type OwnerState } from './owner-state.ts';

const ok = <T>(value: T): ApiResult<T> => ({ ok: true, value });
const network: ApiResult<never> = { ok: false, error: { kind: 'network' } };
const wrong: ApiResult<never> = { ok: false, error: { kind: 'unauthorized' } };

const run = (state: OwnerState, ...events: OwnerEvent[]): OwnerState =>
	events.reduce(reduce, state);

const robin: Owner = { name: 'Robin Hart' };
const signedOut = run(CHECKING, { kind: 'checked', result: ok({ kind: 'none' }) });
const signedIn = run(CHECKING, { kind: 'checked', result: ok({ kind: 'owner', owner: robin }) });

describe('reduce', () => {
	it('follows the session check', () => {
		expect(signedIn).toEqual({ kind: 'signed-in', owner: robin, logout: { kind: 'idle' } });
		expect(signedOut).toEqual({ kind: 'signed-out', login: { kind: 'idle' } });
		// A failed check shows the login form with the error, as a failed login would.
		expect(run(CHECKING, { kind: 'checked', result: network })).toEqual({
			kind: 'signed-out',
			login: { kind: 'failed', error: { kind: 'network' } }
		});
	});

	it('logs in, keeps a wrong password as an error, and retries', () => {
		const sending = run(signedOut, { kind: 'login-sent' });
		expect(sending).toEqual({ kind: 'signed-out', login: { kind: 'sending' } });
		const failed = run(sending, { kind: 'login-answered', result: wrong });
		expect(failed).toEqual({ kind: 'signed-out', login: { kind: 'failed', error: wrong.error } });
		expect(
			run(failed, { kind: 'login-sent' }, { kind: 'login-answered', result: ok(robin) })
		).toEqual(signedIn);
		// A deployment without a name: signed in all the same, with nothing to prefill.
		expect(run(sending, { kind: 'login-answered', result: ok({ name: null }) })).toEqual({
			kind: 'signed-in',
			owner: { name: null },
			logout: { kind: 'idle' }
		});
	});

	it('logs out, and stays in when the server cannot be reached', () => {
		const out = run(
			signedIn,
			{ kind: 'logout-sent' },
			{ kind: 'logout-answered', result: ok(undefined) }
		);
		expect(out).toEqual(signedOut);
		// An expired session is logged out already.
		expect(
			run(signedIn, { kind: 'logout-sent' }, { kind: 'logout-answered', result: wrong })
		).toEqual(signedOut);
		expect(
			run(signedIn, { kind: 'logout-sent' }, { kind: 'logout-answered', result: network })
		).toEqual({
			kind: 'signed-in',
			owner: robin,
			logout: { kind: 'failed', error: { kind: 'network' } }
		});
	});

	it('signs out when an owner request is rejected', () => {
		expect(run(signedIn, { kind: 'rejected' })).toEqual(signedOut);
	});

	it('ignores events that do not fit the state', () => {
		expect(run(signedIn, { kind: 'checked', result: ok({ kind: 'none' }) })).toEqual(signedIn);
		expect(run(signedOut, { kind: 'login-answered', result: ok(robin) })).toEqual(signedOut);
		expect(run(CHECKING, { kind: 'login-sent' })).toEqual(CHECKING);
		expect(run(signedOut, { kind: 'logout-sent' })).toEqual(signedOut);
	});
});

describe('hostOffer', () => {
	it('says nothing while the session check runs', () => {
		expect(hostOffer(CHECKING, false)).toEqual({ kind: 'none' });
	});

	it('joins the owner as host without a password', () => {
		expect(hostOffer(signedIn, false)).toEqual({ kind: 'host' });
		expect(hostOffer(signedIn, true)).toEqual({ kind: 'host' });
	});

	it('shows guests only a link, and the form once it is opened', () => {
		expect(hostOffer(signedOut, false)).toEqual({ kind: 'link' });
		expect(hostOffer(signedOut, true)).toEqual({ kind: 'form' });
	});

	it('shows no password field after a failed session check until the link is opened', () => {
		const failed = run(CHECKING, { kind: 'checked', result: network });
		expect(hostOffer(failed, false)).toEqual({ kind: 'link' });
		expect(hostOffer(failed, true)).toEqual({ kind: 'form' });
	});
});
