import { describe, expect, it } from 'vitest';
import { describeApiError, type ApiErrorWording } from './api-messages.ts';

const WORDING: ApiErrorWording = {
	action: 'create a meeting',
	rateLimited: 'new meetings',
	unauthorized: 'Your login has expired. Log in again.'
};

describe('describeApiError', () => {
	it('takes the parts that differ from the wording', () => {
		expect(describeApiError({ kind: 'unauthorized' }, WORDING)).toBe(WORDING.unauthorized);
		expect(describeApiError({ kind: 'rate-limited' }, WORDING)).toBe(
			'Too many new meetings in a short time. Wait a minute, then try again.'
		);
		expect(describeApiError({ kind: 'server', status: 500 }, WORDING)).toBe(
			'The server could not create a meeting (error 500). Try again.'
		);
	});

	it('says an old Worker needs an update when the route is missing', () => {
		expect(describeApiError({ kind: 'server', status: 404 }, WORDING)).toBe(
			'This server cannot create a meeting yet. Update the Worker, then try again.'
		);
	});

	it('words network and malformed failures the same on every page', () => {
		expect(describeApiError({ kind: 'network' }, WORDING)).toMatch(/could not be reached/);
		expect(describeApiError({ kind: 'malformed' }, WORDING)).toMatch(/does not understand/);
	});
});
