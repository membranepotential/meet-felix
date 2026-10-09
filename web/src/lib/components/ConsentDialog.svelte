<script lang="ts">
	/**
	 * The room records and we have not consented (docs/recording/README.md, D15). Until the
	 * user accepts, they stay outside the room or, if already in, muted and not recorded.
	 */
	let { onaccept, onleave }: { onaccept: () => void; onleave: () => void } = $props();

	function open(dialog: HTMLDialogElement) {
		dialog.showModal();
		return () => dialog.close();
	}
</script>

<!-- Escape must not dismiss it: the user has to choose. -->
<dialog
	class="dialog"
	aria-labelledby="consent-title"
	{@attach open}
	oncancel={(e) => e.preventDefault()}
>
	<h2 id="consent-title">This meeting is being recorded</h2>
	<p>
		Everyone’s voice is recorded and turned into a written transcript after the meeting. Video is
		not recorded. Everyone in the meeting gets the transcript link at the end.
	</p>
	<p>Until you accept, you are muted and your voice is not recorded.</p>
	<div class="dialog-actions">
		<button class="button quiet" type="button" onclick={onleave}>Leave</button>
		<button class="button primary" type="button" onclick={onaccept}>Accept</button>
	</div>
</dialog>
