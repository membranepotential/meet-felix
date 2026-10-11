/**
 * Browser-local persistence. Every access is wrapped because storage throws in some
 * private modes and when site data is blocked; the app must work without it.
 */

const DISPLAY_NAME = 'meet-felix:name';
/**
 * Secrets earlier versions kept: the Worker's create secret, and one host token per meeting.
 * The owner's session cookie replaces both; `dropLegacySecrets` removes the old copies. The
 * create secret is the owner's password in plain text (D24), so the cleanup stays until every
 * browser that created a meeting before 2026-10-09 has opened the start page once.
 */
const LEGACY_CREATE_SECRET = 'meet-felix:create-secret';
const LEGACY_HOST_TOKEN_PREFIX = 'meet-felix:host:';

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

export const loadDisplayName = (): string => read(DISPLAY_NAME) ?? '';
export const saveDisplayName = (name: string): void => write(DISPLAY_NAME, name);

export function dropLegacySecrets(): void {
	try {
		const keys = Object.keys(localStorage).filter(
			(key) => key === LEGACY_CREATE_SECRET || key.startsWith(LEGACY_HOST_TOKEN_PREFIX)
		);
		for (const key of keys) localStorage.removeItem(key);
	} catch {
		// Storage unavailable: nothing was stored.
	}
}
