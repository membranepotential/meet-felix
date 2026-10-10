/**
 * End-to-end tests (docs/TESTING.md): a protocol layer that drives rooms over raw WebSockets,
 * and a browser layer with fake devices in Chromium and Firefox, both against a local
 * `wrangler dev` with the mock SFU; the `@recorder` protocol test, which needs the real Workers
 * AI; and `@real` browser tests against the real Realtime SFU.
 */
import { defineConfig, devices } from '@playwright/test';
import type { StackKind } from './e2e/fixtures/stack.ts';

const CI = process.env.CI !== undefined && process.env.CI !== '';

const chromiumMedia = {
	args: [
		// Two cameras and three mics, so a test can switch devices (Firefox fakes one of each).
		'--use-fake-device-for-media-stream=device-count=2',
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
		{
			name: 'protocol',
			testDir: './e2e/protocol',
			grepInvert: /@recorder/,
			use: { stackKind: 'mock' }
		},
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
			name: 'recorder',
			testDir: './e2e/protocol',
			grep: /@recorder/,
			// About 40 s of speech, then the transcript job, which waits on Workers AI.
			timeout: 300_000,
			use: { stackKind: 'recorder' }
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
