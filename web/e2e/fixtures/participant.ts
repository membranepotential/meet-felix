/**
 * One person in a browser: a context of their own (the owner's has the session cookie), a page,
 * and a log of the signalling frames the page sent and received, read from the browser's
 * WebSocket so tests can assert what the client did without touching its code.
 */
import {
	expect,
	type Browser,
	type BrowserContext,
	type Locator,
	type Page
} from '@playwright/test';
import type { ClientMsg, MeetingId, ServerMsg } from '../../src/lib/protocol.ts';
import type { OwnerSession } from './api.ts';

export type Frame =
	{ at: number; dir: 'sent'; msg: ClientMsg } | { at: number; dir: 'received'; msg: ServerMsg };

type Sent<T extends ClientMsg['type']> = Extract<ClientMsg, { type: T }>;
type Received<T extends ServerMsg['type']> = Extract<ServerMsg, { type: T }>;

declare global {
	interface Window {
		/** Every `RTCPeerConnection` the page created, oldest first (see `trackPeerConnections`). */
		__e2ePeerConnections?: RTCPeerConnection[];
		/** When a peer connection reached `connected`, by the wall clock (see `trackPeerConnections`). */
		__e2eConnected?: Connected[];
	}
}

/** A peer connection that reached `connectionState` "connected". */
export type Connected = {
	/** `Date.now()` in the page; the browsers run on the test's machine, so it is the test's clock. */
	at: number;
	/** Whether it is a publish connection (one of its transceivers sends). */
	publishes: boolean;
};

/**
 * Runs in the page before its scripts: keeps every peer connection for `getStats()` and faults,
 * and notes when each one connects.
 */
function trackPeerConnections(): void {
	const created: RTCPeerConnection[] = [];
	const connected: Connected[] = [];
	window.__e2ePeerConnections = created;
	window.__e2eConnected = connected;
	const Original = window.RTCPeerConnection;
	window.RTCPeerConnection = class extends Original {
		constructor(config?: RTCConfiguration) {
			super(config);
			created.push(this);
			this.addEventListener('connectionstatechange', () => {
				if (this.connectionState !== 'connected') return;
				connected.push({
					at: Date.now(),
					publishes: this.getTransceivers().some((t) => t.direction === 'sendonly')
				});
			});
		}
	};
}

const isJson = (payload: string | Buffer): payload is string =>
	typeof payload === 'string' &&
	payload.startsWith('{') &&
	payload !== '{"type":"ping"}' &&
	payload !== '{"type":"pong"}';

export class Participant {
	readonly label: string;
	readonly context: BrowserContext;
	readonly page: Page;
	readonly frames: Frame[] = [];
	/** WebSockets the page opened to the room. */
	sockets = 0;

	private constructor(label: string, context: BrowserContext, page: Page) {
		this.label = label;
		this.context = context;
		this.page = page;
		page.on('websocket', (socket) => {
			if (!socket.url().includes('/ws/')) return;
			this.sockets += 1;
			socket.on('framesent', ({ payload }) => {
				if (isJson(payload))
					this.frames.push({ at: Date.now(), dir: 'sent', msg: JSON.parse(payload) as ClientMsg });
			});
			socket.on('framereceived', ({ payload }) => {
				if (isJson(payload)) {
					this.frames.push({
						at: Date.now(),
						dir: 'received',
						msg: JSON.parse(payload) as ServerMsg
					});
				}
			});
		});
	}

	/** A new context and page; with `owner`, the context carries the owner's session cookie. */
	static async open(
		browser: Browser,
		baseURL: string,
		label: string,
		owner: OwnerSession | null
	): Promise<Participant> {
		const context = await browser.newContext({ baseURL, viewport: { width: 1280, height: 800 } });
		if (owner) {
			await context.addCookies([{ name: 'owner', value: owner.value, url: baseURL }]);
		}
		await context.addInitScript(trackPeerConnections);
		return new Participant(label, context, await context.newPage());
	}

	sent<T extends ClientMsg['type']>(type: T): Sent<T>[] {
		return this.frames.flatMap((f) =>
			f.dir === 'sent' && f.msg.type === type ? [f.msg as Sent<T>] : []
		);
	}

	received<T extends ServerMsg['type']>(type: T): Received<T>[] {
		return this.frames.flatMap((f) =>
			f.dir === 'received' && f.msg.type === type ? [f.msg as Received<T>] : []
		);
	}

	/** Waits until the page has sent a `type` message that `matches`. */
	async waitForSent<T extends ClientMsg['type']>(
		type: T,
		matches: (msg: Sent<T>) => boolean = () => true,
		timeout = 10_000
	): Promise<Sent<T>> {
		await expect.poll(() => this.sent(type).some(matches), { timeout }).toBe(true);
		return this.sent(type).find(matches) as Sent<T>;
	}

	/** Waits until the page has received a `type` message that `matches`. */
	async waitForReceived<T extends ServerMsg['type']>(
		type: T,
		matches: (msg: Received<T>) => boolean = () => true,
		timeout = 10_000
	): Promise<Received<T>> {
		await expect.poll(() => this.received(type).some(matches), { timeout }).toBe(true);
		return this.received(type).find(matches) as Received<T>;
	}

	// ---- The UI, by the roles and texts it shows

	/** Opens the meeting link and fills the name field (the owner's comes prefilled). */
	async openPreJoin(meeting: MeetingId, name?: string): Promise<void> {
		await this.page.goto(`/${meeting}`);
		const field = this.page.getByLabel('Your name');
		if (name !== undefined) await field.fill(name);
		await expect(this.joinButton).toBeEnabled({ timeout: 15_000 });
	}

	/** The pre-join's submit button; its text names the recording consent. */
	get joinButton(): Locator {
		return this.page.locator('form button[type=submit]').first();
	}

	/** Presses Join on the pre-join screen. */
	async join(): Promise<void> {
		await this.joinButton.click();
	}

	/** The in-meeting view. */
	get meeting(): Locator {
		return this.page.getByTestId('meeting');
	}

	/** Every participant's tile, in any layout (grid, strip beside a screen, chip). */
	get tiles(): Locator {
		return this.page.getByTestId('tile');
	}

	/** The tile whose name is `name` exactly, e.g. "Guest" or "Felix Peppert (you)". */
	tile(name: string): Locator {
		return this.tiles.filter({
			has: this.page
				.getByTestId('tile-name')
				.filter({ hasText: new RegExp(`^${escapeRegExp(name)}$`) })
		});
	}

	/** The notification pill above the control bar that contains `text`. */
	pill(text: string): Locator {
		return this.page.getByTestId('pill').filter({ hasText: text });
	}

	/** The transcript link on the page shown after a recorded meeting ended. */
	get transcriptLink(): Locator {
		return this.page.getByTestId('transcript-link');
	}

	control(name: string): Locator {
		return this.page
			.getByRole('toolbar', { name: 'Meeting controls' })
			.getByRole('button', { name, exact: true });
	}

	/** Opens the host group of the control bar if it is folded, and returns `name` in it. */
	async hostControl(name: string): Promise<Locator> {
		const button = this.control(name);
		if (!(await button.isVisible())) await this.control('Host').click();
		return button;
	}

	/** Ends the meeting for everyone, through the confirmation dialog. */
	async endForAll(): Promise<void> {
		await (await this.hostControl('End for all')).click();
		await this.page.getByRole('button', { name: 'End meeting' }).click();
	}

	/**
	 * Makes the page's subscribe peer connection report `failed`, as a browser does when the
	 * SFU stops answering: the newest open connection that sends nothing gets `connectionState` "failed"
	 * and a `connectionstatechange` event. Returns how many connections the page had then.
	 */
	failSubscribeConnection(): Promise<number> {
		return this.page.evaluate(() => {
			const all = window.__e2ePeerConnections ?? [];
			// The subscribe connection only receives; the publish connection sends.
			const subscriber = all.findLast(
				(pc) =>
					pc.signalingState !== 'closed' &&
					pc.getTransceivers().every((t) => t.direction !== 'sendonly')
			);
			if (!subscriber) throw new Error('no open subscribe connection');
			Object.defineProperty(subscriber, 'connectionState', {
				get: () => 'failed',
				configurable: true
			});
			subscriber.dispatchEvent(new Event('connectionstatechange'));
			return all.length;
		});
	}

	/** Every time one of the page's peer connections reached `connected`, oldest first. */
	connected(): Promise<Connected[]> {
		return this.page.evaluate(() => window.__e2eConnected ?? []);
	}

	close(): Promise<void> {
		return this.context.close();
	}
}

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
