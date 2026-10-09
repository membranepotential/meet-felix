/**
 * The room closes a socket that sends no `{"type":"ping"}` for about a minute (code 1001) and
 * keeps one that does. Takes two minutes, so it runs only with `E2E_SLOW=1`.
 */
import { body } from '../fixtures/api.ts';
import { PING } from '../fixtures/room-client.ts';
import { expect, test } from '../fixtures/test.ts';

test('a silent socket is closed with 1001, a pinging one stays', async ({ api, owner, rooms }) => {
	test.skip(process.env.E2E_SLOW !== '1', 'set E2E_SLOW=1 for the two-minute keepalive test');
	test.setTimeout(180_000);
	const { meeting } = await body<{ meeting: string }>(
		await api.newMeeting(owner.headers, { recording: 'hostDecides' })
	);
	const [silent, pinging] = await Promise.all([rooms.open(meeting), rooms.open(meeting)]);
	const timer = setInterval(() => pinging.sendRaw(PING), 20_000);
	try {
		expect((await silent.closes(130_000)).code).toBe(1001);
		expect(pinging.isOpen).toBe(true);
	} finally {
		clearInterval(timer);
	}
});
