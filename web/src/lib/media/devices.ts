/**
 * Device enumeration. Labels are empty until the page holds a media permission, so
 * enumerate again after the first successful `getUserMedia`.
 */

export interface Device {
	id: string;
	label: string;
}

export interface DeviceLists {
	microphones: Device[];
	cameras: Device[];
}

const EMPTY: DeviceLists = { microphones: [], cameras: [] };

export async function listDevices(): Promise<DeviceLists> {
	if (!navigator.mediaDevices?.enumerateDevices) return EMPTY;
	let infos: MediaDeviceInfo[];
	try {
		infos = await navigator.mediaDevices.enumerateDevices();
	} catch {
		return EMPTY;
	}
	const of = (kind: MediaDeviceKind) =>
		infos
			// Hide entries without an id: they appear before permission is granted.
			.filter((d) => d.kind === kind && d.deviceId !== '')
			.map((d, i) => ({ id: d.deviceId, label: d.label || fallbackLabel(kind, i) }));
	return { microphones: of('audioinput'), cameras: of('videoinput') };
}

function fallbackLabel(kind: MediaDeviceKind, index: number): string {
	const noun = kind === 'audioinput' ? 'Microphone' : 'Camera';
	return `${noun} ${index + 1}`;
}

/** Calls `onChange` when devices are plugged in or removed. Returns the unsubscribe. */
export function watchDevices(onChange: () => void): () => void {
	const media = navigator.mediaDevices;
	if (!media) return () => {};
	media.addEventListener('devicechange', onChange);
	return () => media.removeEventListener('devicechange', onChange);
}
