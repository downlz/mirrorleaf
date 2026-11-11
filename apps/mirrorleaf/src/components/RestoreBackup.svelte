<script lang="ts">
  // Restoring a backup (E9): what it holds joins what this device has, by the
  // same merge sync uses, so nothing here is replaced. An encrypted backup made
  // with an earlier account key needs that kit's words.
  import { api, type PickedBackup } from '../lib/api';
  import { app } from '../lib/state.svelte';

  let { picked, onclose }: { picked: PickedBackup; onclose: () => void } = $props();

  let words = $state('');
  let working = $state(false);
  let error = $state('');

  async function restore() {
    working = true;
    error = '';
    try {
      await api.restoreBackup(picked.path, picked.info.needs_kit ? words : null);
      app.notify('Backup restored.');
      await app.refresh();
      await app.loadStatus();
      onclose();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      working = false;
    }
  }

  function cancel() {
    void api.forgetBackup(picked.path).catch(() => {});
    onclose();
  }
</script>

<h2 class="question">Restore the backup from {picked.info.made}?</h2>
<p class="hint">
  Anything in it that this {app.deviceWord} doesn’t have comes back, and reaches your other devices as
  they sync. Nothing here is replaced.
</p>
{#if picked.info.needs_kit}
  <p class="hint">
    It was encrypted with an earlier recovery kit. Enter the 12 words of the kit you had when you
    made it.
  </p>
  <label class="field">
    <span>Recovery words</span>
    <textarea
      id="backup-words"
      rows="3"
      autocomplete="off"
      autocapitalize="none"
      spellcheck="false"
      bind:value={words}></textarea>
  </label>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}
<div class="actions">
  <button
    class="primary"
    disabled={working || (picked.info.needs_kit && words.trim().split(/\s+/).length < 12)}
    onclick={restore}>{working ? 'Restoring…' : 'Restore'}</button
  >
  <button disabled={working} onclick={cancel}>Cancel</button>
</div>
