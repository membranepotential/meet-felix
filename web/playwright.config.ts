/**
 * End-to-end tests (docs/TESTING.md): a protocol layer that drives rooms over raw WebSockets,
 * and a browser layer with fake devices in Chromium and Firefox, both against a local
 * `wrangler dev` with the mock SFU; and `@real` browser tests against the real Realtime SFU.
 */
import { defineConfig, devices } from '@playwright/test';
import type { StackKind } from './e2e/fixtures/stack.ts';

const CI = process.env.CI !== undefined && process.env.CI !== '';

const chromiumMedia = {
	args: [
		'--use-fake-device-for-media-stream',
		'--use-fake-ui-for-media-stream',
		'--autoplay-policy=no-user-gesture-required'
	]
};

const firefoxMedia = {
	firefoxUserPrefs: {
		'media.navigator.streams.fake': true,
		'media.navigator.permission.disabled': true,
		'media.autoplay.default': 0
	}
};

export default defineConfig<{ stackKind: StackKind }>({
	testDir: './e2e',
	outputDir: './e2e/.artifacts/results',
	globalSetup: './e2e/global-setup.ts',
	timeout: 60_000,
	expect: { timeout: 10_000 },
	forbidOnly: CI,
	retries: CI ? 1 : 0,
	reporter: process.env.E2E_HTML_REPORT
		? [['list'], ['html', { outputFolder: './e2e/.artifacts/report', open: 'never' }]]
		: 'list',
	use: {
		trace: 'on-first-retry',
		screenshot: 'only-on-failure'
	},
	projects: [
		{ name: 'protocol', testDir: './e2e/protocol', use: { stackKind: 'mock' } },
		{
			name: 'chromium',
			testDir: './e2e/browser',
			grepInvert: /@real/,
			use: { ...devices['Desktop Chrome'], stackKind: 'mock', launchOptions: chromiumMedia }
		},
		{
			name: 'firefox',
			testDir: './e2e/browser',
			grepInvert: /@real/,
			use: { ...devices['Desktop Firefox'], stackKind: 'mock', launchOptions: firefoxMedia }
		},
		{
			name: 'real',
			testDir: './e2e/browser',
			grep: /@real/,
			timeout: 240_000,
			use: { ...devices['Desktop Chrome'], stackKind: 'real', launchOptions: chromiumMedia }
		}
	]
});
