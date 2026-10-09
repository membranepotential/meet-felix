/** Short-lived notices, shown as pills just above the control bar. */

export type ToastTone = 'info' | 'alert';

export interface Toast {
	id: number;
	text: string;
	tone: ToastTone;
}

/** How long a toast stays. Alerts stay longer, so there is time to read what went wrong. */
const DURATION_MS: Record<ToastTone, number> = { info: 5_000, alert: 8_000 };

/** More would cover the stage; a new toast drops the oldest. */
export const MAX_TOASTS = 3;

export class Toasts {
	items = $state.raw<Toast[]>([]);
	#nextId = 0;

	show(text: string, tone: ToastTone = 'info'): void {
		const id = this.#nextId++;
		this.items = [...this.items, { id, text, tone }].slice(-MAX_TOASTS);
		setTimeout(() => this.dismiss(id), DURATION_MS[tone]);
	}

	dismiss(id: number): void {
		this.items = this.items.filter((t) => t.id !== id);
	}
}
