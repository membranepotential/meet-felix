import type { ApiError } from './api.ts';

/** The words a page uses for a failed `/api/*` request: the parts that differ between requests. */
export interface ApiErrorWording {
	/** What the request did, after "could not": "create a meeting". */
	action: string;
	/** What was too frequent, in "Too many … in a short time": "new meetings". */
	rateLimited: string;
	/** What an `unauthorized` reply means on this page. */
	unauthorized: string;
}

/** The sentence that tells the user why `error` happened and what to do. */
export function describeApiError(error: ApiError, wording: ApiErrorWording): string {
	switch (error.kind) {
		case 'unauthorized':
			return wording.unauthorized;
		case 'rate-limited':
			return `Too many ${wording.rateLimited} in a short time. Wait a minute, then try again.`;
		case 'network':
			return 'The server could not be reached. Check your connection, then try again.';
		case 'server':
			// A 404 without the Worker's error body comes from a Worker without the route: the
			// static app is newer than the Worker.
			return error.status === 404 && error.code === null
				? `This server cannot ${wording.action} yet. Update the Worker, then try again.`
				: `The server could not ${wording.action} (error ${error.status}). Try again.`;
		case 'malformed':
			return 'The server sent a reply this page does not understand. Try again.';
	}
}
