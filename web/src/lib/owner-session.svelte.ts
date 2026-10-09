/**
 * The owner's session in this browser (D32), as reactive state for the pages: checks the
 * session cookie, logs in and out. Each page makes its own and calls `check()` on mount.
 */
import { isOwner, login, logout } from './api.ts';
import { CHECKING, reduce, type OwnerEvent, type OwnerState } from './owner-state.ts';

export class OwnerSession {
	state: OwnerState = $state.raw(CHECKING);
	signedIn = $derived(this.state.kind === 'signed-in');

	async check(): Promise<void> {
		this.#apply({ kind: 'checked', result: await isOwner() });
	}

	/** Returns whether the login succeeded. The password is not kept. */
	async login(password: string): Promise<boolean> {
		this.#apply({ kind: 'login-sent' });
		const result = await login(password);
		this.#apply({ kind: 'login-answered', result });
		return result.ok;
	}

	async logout(): Promise<void> {
		this.#apply({ kind: 'logout-sent' });
		this.#apply({ kind: 'logout-answered', result: await logout() });
	}

	/** An owner request came back `unauthorized`: show the login form again. */
	rejected(): void {
		this.#apply({ kind: 'rejected' });
	}

	#apply(event: OwnerEvent): void {
		this.state = reduce(this.state, event);
	}
}
