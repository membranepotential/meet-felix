import { readFileSync } from 'node:fs';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { initSync } from './wasm/meeting_core.js';
import { backoffDelay, decodeFrame, SignalingClient, type ConnectionStatus } from './signaling.ts';

beforeAll(() => {
	initSync({ module: readFileSync(new URL('./wasm/meeting_core_bg.wasm', import.meta.url)) });
});

afterEach(() => {
	vi.restoreAllMocks();
});

describe('backoffDelay', () => {
	it('doubles from 500 ms up to a 10 s ceiling, with jitter in the upper half', () => {
		const low = (n: number) => backoffDelay(n, () => 0);
		const high = (n: number) => backoffDelay(n, () => 1);
		expect([1, 2, 3, 4].map(high)).toEqual([500, 1000, 2000, 4000]);
		expect([1, 2, 3].map(low)).toEqual([250, 500, 1000]);
		expect(high(20)).toBe(10_000);
		expect(low(20)).toBe(5_000);
	});
});

describe('decodeFrame', () => {
	it('passes valid messages through and drops the keepalive answer silently', () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		expect(decodeFrame('{"type":"waiting"}')).toEqual({ type: 'waiting' });
		expect(decodeFrame('{"type":"pong"}')).toBeNull();
		expect(warn).not.toHaveBeenCalled();
	});

	it('logs an invalid frame with at most 200 characters of it and ignores it', () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		const raw = `{"type":"welcome","junk":"${'x'.repeat(500)}"}`;
		expect(decodeFrame(raw)).toBeNull();
		expect(warn).toHaveBeenCalledOnce();
		expect(warn.mock.calls[0]?.at(-1)).toBe(raw.slice(0, 200));
	});
});

/** A WebSocket the test opens, feeds and closes by hand. */
class FakeSocket extends EventTarget {
	static readonly OPEN = 1;
	static all: FakeSocket[] = [];
	readyState = 0;
	readonly sent: string[] = [];

	constructor(readonly url: string) {
		super();
		FakeSocket.all.push(this);
	}

	send(frame: string): void {
		this.sent.push(frame);
	}

	close(): void {
		this.readyState = 3;
	}

	open(): void {
		this.readyState = FakeSocket.OPEN;
		this.dispatchEvent(new Event('open'));
	}

	receive(data: string): void {
		this.dispatchEvent(Object.assign(new Event('message'), { data }));
	}

	closedBy(code: number, reason = ''): void {
		this.readyState = 3;
		this.dispatchEvent(Object.assign(new Event('close'), { code, reason }));
	}
}

describe('SignalingClient', () => {
	const listeners = { addEventListener: () => {}, removeEventListener: () => {} };
	let statuses: ConnectionStatus[];
	let client: SignalingClient;

	const latest = (): FakeSocket => {
		const socket = FakeSocket.all.at(-1);
		if (socket === undefined) throw new Error('no socket');
		return socket;
	};
	const waits = (): number[] => statuses.flatMap((s) => (s.kind === 'waiting' ? [s.attempt] : []));

	beforeEach(() => {
		vi.useFakeTimers();
		FakeSocket.all = [];
		vi.stubGlobal('WebSocket', FakeSocket);
		vi.stubGlobal('window', { ...listeners, location: { protocol: 'https:', host: 'meet.test' } });
		vi.stubGlobal('document', { ...listeners, visibilityState: 'visible' });
		statuses = [];
		client = new SignalingClient('amber-otter-ocean', {
			onOpen: () => {},
			onMessage: () => {},
			onStatus: (status) => statuses.push(status)
		});
		client.connect();
	});

	afterEach(() => {
		client.close();
		vi.useRealTimers();
		vi.unstubAllGlobals();
	});

	it('backs off further while the room refuses the join, and starts over once it takes it', async () => {
		for (let i = 0; i < 3; i++) {
			latest().open();
			latest().receive('{"type":"error","code":"sfuUnavailable"}');
			latest().closedBy(1013, 'the media server is not reachable');
			await vi.runOnlyPendingTimersAsync();
		}
		expect(waits()).toEqual([1, 2, 3]);
		// The room takes the join; a later lost connection retries soon again.
		latest().open();
		latest().receive('{"type":"waiting"}');
		latest().closedBy(1006);
		const last = statuses.at(-1);
		expect(last?.kind === 'waiting' && last.retryInMs).toBeLessThanOrEqual(500);
	});

	it('keeps backing off when the room asks a host for consent before the SFU refuses the join', async () => {
		for (let i = 0; i < 3; i++) {
			latest().open();
			latest().receive('{"type":"consentRequired"}');
			latest().receive('{"type":"error","code":"sfuUnavailable"}');
			latest().closedBy(1013, 'the media server is not reachable');
			await vi.runOnlyPendingTimersAsync();
		}
		expect(waits()).toEqual([1, 2, 3]);
	});

	it('stops for good on a final close code, and asks for a reload on an outdated one', () => {
		latest().open();
		latest().closedBy(4002, 'The meeting has ended.');
		expect(statuses.at(-1)).toEqual({
			kind: 'dismissed',
			after: 'stop',
			reason: 'The meeting has ended.'
		});
		expect(FakeSocket.all).toHaveLength(1);

		client = new SignalingClient('amber-otter-ocean', {
			onOpen: () => {},
			onMessage: () => {},
			onStatus: (status) => statuses.push(status)
		});
		client.connect();
		latest().open();
		latest().closedBy(4004, 'This page is out of date. Reload it to join the meeting.');
		expect(statuses.at(-1)).toMatchObject({ kind: 'dismissed', after: 'reload' });
	});
});
