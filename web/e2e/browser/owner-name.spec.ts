/**
 * The owner's name comes from the deployment (`OWNER_NAME`), which may set none: then
 * `GET /api/me` answers `{"name": null}` and the pre-join has nothing to prefill.
 */
import { createMeeting } from '../fixtures/api.ts';
import { expect, test } from '../fixtures/test.ts';

test('the owner’s pre-join leaves the name empty when the deployment names no owner', async ({
	api,
	owner,
	host
}) => {
	const meeting = await createMeeting(api, owner.headers);
	// The stack names an owner; this page sees the answer of a deployment that names none.
	await host.page.route('**/api/me', async (route) => {
		const response = await route.fetch();
		await route.fulfill({ response, json: { name: null } });
	});
	await host.page.goto(`/${meeting}`);
	await expect(host.page.getByText('You join as host')).toBeVisible();
	const name = host.page.getByLabel('Your name');
	await expect(name).toHaveValue('');
	await expect(host.joinButton).toBeDisabled();

	// The owner types a name and can join.
	await name.fill('Sam');
	await expect(host.joinButton).toBeEnabled();
});
