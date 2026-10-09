/** Short-lived notices shown above the meeting. */

export type ToastTone = 'info' | 'alert';

export interface Toast {
	id: number;
	text: string;
	tone: ToastTone;
}

export class Toasts {
	items = $state.raw<Toast[]>([]);
	#nextId = 0;

	show(text: string, tone: ToastTone = 'info', durationMs = 5000): void {
		const id = this.#nextId++;
		this.items = [...this.items, { id, text, tone }];
		setTimeout(() => this.dismiss(id), durationMs);
	}

	dismiss(id: number): void {
		this.items = this.items.filter((t) => t.id !== id);
	}
}
