<script lang="ts">
  // Another device removed this one from the account (E3): it no longer
  // syncs. Erasing it lets it start again, as a new device.
  import { api } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Logo from './Logo.svelte';

  let busy = $state(false);

  async function erase() {
    busy = true;
    // The app starts afresh once the device is erased.
    await app.run(() => api.signOut(true));
    busy = false;
  }
</script>

<main class="welcome" data-tauri-drag-region>
  <div class="welcome-brand">
    <Logo size={56} />
    <h1>This {app.deviceWord} was removed</h1>
    <p>
      Another of your devices removed it from your account, so it no longer syncs. Your notes are
      safe on your other devices.
    </p>
  </div>
  <div class="welcome-choices">
    <button class="primary" disabled={busy} onclick={erase}>Erase it and start again</button>
  </div>
</main>
