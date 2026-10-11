/** The owner's session: `POST /api/login`, `GET /api/me`, `POST /api/logout` (D32). */
import { cookieOf } from '../fixtures/api.ts';
import { OWNER_NAME } from '../fixtures/stack.ts';
import { expect, test } from '../fixtures/test.ts';

const SESSION_COOKIE =
	/^owner=\d+\.[0-9a-f]{64}; HttpOnly; Secure; SameSite=Lax; Path=\/; Max-Age=2592000$/;

test('a wrong password gets 401 and no cookie', async ({ api }) => {
	const response = await api.login('wrong');
	expect(response.status).toBe(401);
	expect(cookieOf(response)).toBe('');
});

test('a login without a password gets 400', async ({ api }) => {
	expect((await api.loginWith('{}')).status).toBe(400);
});

test('the right password gets the session cookie and the owner’s name', async ({ api, stack }) => {
	const response = await api.login(stack.ownerPassword);
	expect(response.status).toBe(200);
	expect(response.headers.getSetCookie()[0]).toMatch(SESSION_COOKIE);
	expect(await response.json()).toEqual({ name: OWNER_NAME });
});

test('GET /api/me names the owner with the cookie and answers 401 without or with a forged one', async ({
	api,
	owner
}) => {
	const me = await api.me(owner.headers);
	expect(me.status).toBe(200);
	expect(me.headers.get('cache-control')).toBe('no-store');
	expect(await me.json()).toEqual({ name: OWNER_NAME });
	expect((await api.me({})).status).toBe(401);
	expect((await api.me({ cookie: `owner=99999999999999.${'0'.repeat(64)}` })).status).toBe(401);
});

test('POST /api/logout answers 204 and clears the cookie', async ({ api }) => {
	const response = await api.logout();
	expect(response.status).toBe(204);
	expect(response.headers.getSetCookie()[0]).toMatch(/^owner=;.*Max-Age=0$/);
});
