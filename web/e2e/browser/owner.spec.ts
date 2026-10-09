/** The owner's pages: login on `/`, creating a meeting, past meetings, logout. */
import { createMeeting } from '../fixtures/api.ts';
import { startCall } from '../fixtures/call.ts';
import { expect, test } from '../fixtures/test.ts';

test('the owner logs in, creates a meeting and logs out', async ({ people, stack }) => {
	const owner = await people.open('owner');
	const { page } = owner;
	await page.goto('/');
	await expect(page.getByRole('heading', { name: 'Log in' })).toBeVisible();

	await page.getByLabel('Password').fill('not-the-password');
	await page.getByRole('button', { name: 'Log in' }).click();
	await expect(page.getByRole('alert')).toHaveText('That password is wrong.');

	await page.getByLabel('Password').fill(stack.ownerPassword);
	await page.getByRole('button', { name: 'Log in' }).click();
	await expect(page.getByRole('heading', { name: 'New meeting' })).toBeVisible();
	await expect(page.getByRole('switch')).toBeChecked();

	await page.getByRole('button', { name: 'New meeting' }).click();
	const joinNow = page.getByRole('link', { name: 'Join now' });
	await expect(joinNow).toHaveAttribute('href', /^\/[a-z]+-[a-z]+-[a-z]+$/);

	await page.goto('/');
	await page.getByRole('button', { name: 'Log out' }).click();
	await expect(page.getByRole('heading', { name: 'Log in' })).toBeVisible();
	expect(await page.evaluate(async () => (await fetch('/api/me')).status)).toBe(401);
});

test('past meetings show each meeting’s phase and the transcript link once ended', async ({
	api,
	host,
	guest,
	owner
}) => {
	const scheduled = await createMeeting(api, owner.headers, 'fromStart');
	const meeting = await createMeeting(api, owner.headers, 'fromStart');
	await startCall(meeting, host, [{ who: guest, name: 'Guest' }]);

	const list = await host.context.newPage();
	const item = (words: string) => list.locator('li.item', { hasText: words });
	await list.goto('/meetings');
	await expect(item(scheduled).locator('.state')).toHaveText('Scheduled');
	await expect(item(meeting).locator('.state')).toHaveText('Live');

	await host.endForAll();
	await expect(host.page.getByRole('heading', { name: 'The meeting has ended' })).toBeVisible();
	await expect
		.poll(async () => {
			await list.reload();
			return item(meeting).locator('.state').textContent();
		})
		.toBe('Ended');
	await expect(item(meeting).getByRole('link', { name: 'Transcript' })).toHaveAttribute(
		'href',
		/^\/t\/[0-9a-f]{32}$/
	);
});
