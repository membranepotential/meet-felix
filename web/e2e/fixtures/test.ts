/**
 * The e2e `test`: Playwright's, plus fixtures for the local stack.
 *
 * Worker-scoped: `stack` (wrangler dev and the mock or real SFU, one per Playwright worker).
 * Test-scoped: `api`, `owner` (a fresh session cookie), `sfu` (the mock's control client,
 * reset before each test), `rooms` (protocol clients, closed after the test), and for browser
 * tests `people` (participants in their own contexts) with `host` and `guest` shortcuts.
 */
import { test as base } from '@playwright/test';
import type { MeetingId } from '../../src/lib/protocol.ts';
import { Api, loginAsOwner, type Auth, type OwnerSession } from './api.ts';
import type { MockSfu } from './mock-sfu-control.ts';
import { Participant } from './participant.ts';
import { RoomClient } from './room-client.ts';
import { startStack, type Stack, type StackKind } from './stack.ts';

export { expect } from '@playwright/test';

/** Opens protocol clients; the fixture closes them after the test. */
export interface Rooms {
	open(meeting: MeetingId, headers?: Auth): Promise<RoomClient>;
}

/** Opens participants; the fixture closes their contexts after the test. */
export interface People {
	open(label: string, options?: { owner?: boolean }): Promise<Participant>;
}

interface WorkerFixtures {
	/** Which SFU the stack uses; set per project in `playwright.config.ts`. */
	stackKind: StackKind;
	stack: Stack;
}

interface TestFixtures {
	api: Api;
	owner: OwnerSession;
	sfu: MockSfu;
	rooms: Rooms;
	people: People;
	host: Participant;
	guest: Participant;
}

export const test = base.extend<TestFixtures, WorkerFixtures>({
	stackKind: ['mock', { scope: 'worker', option: true }],
	stack: [
		async ({ stackKind }, use, workerInfo) => {
			const stack = await startStack(
				stackKind,
				`${workerInfo.project.name}-w${workerInfo.workerIndex}`
			);
			try {
				await use(stack);
			} finally {
				await stack.stop();
			}
		},
		{ scope: 'worker', timeout: 120_000 }
	],
	baseURL: async ({ stack }, use) => use(stack.baseUrl),
	api: async ({ stack }, use) => use(new Api(stack.baseUrl)),
	owner: async ({ api, stack }, use) => use(await loginAsOwner(api, stack.ownerPassword)),
	sfu: async ({ stack }, use) => {
		if (!stack.sfu) throw new Error('the sfu fixture needs the mock SFU (stackKind "mock")');
		await stack.sfu.reset();
		await use(stack.sfu);
		await stack.sfu.reset();
	},
	rooms: async ({ stack }, use) => {
		const opened: RoomClient[] = [];
		await use({
			async open(meeting, headers) {
				const client = await RoomClient.open(stack.baseUrl, meeting, headers);
				opened.push(client);
				return client;
			}
		});
		for (const client of opened) client.close();
	},
	people: async ({ browser, stack, api }, use) => {
		const opened: Participant[] = [];
		await use({
			async open(label, options) {
				const session = options?.owner ? await loginAsOwner(api, stack.ownerPassword) : null;
				const participant = await Participant.open(browser, stack.baseUrl, label, session);
				opened.push(participant);
				return participant;
			}
		});
		await Promise.all(opened.map((p) => p.close()));
	},
	host: async ({ people }, use) => use(await people.open('host', { owner: true })),
	guest: async ({ people }, use) => use(await people.open('guest'))
});
