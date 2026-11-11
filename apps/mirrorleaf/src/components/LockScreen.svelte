<script lang="ts">
  // The app lock (E4): covers the screen until the device's own unlock passes
  // (Touch ID or the Mac's password; the phone's fingerprint, face or screen
  // lock). Background sync carries on behind it.
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Logo from './Logo.svelte';

  let asking = $state(false);

  const how = $derived(
    app.platform === 'android'
      ? 'Use your fingerprint, face or screen lock.'
      : 'Use Touch ID or your Mac’s password.',
  );

  async function unlock() {
    if (asking) return;
    asking = true;
    const unlocked = await app.run(api.unlock);
    asking = false;
    if (unlocked) app.locked = false;
  }

  onMount(() => {
    // Asks at once while the screen is in sight; the button asks again.
    if (document.visibilityState === 'visible') void unlock();
  });
</script>

<main class="welcome" data-tauri-drag-region>
  <div class="welcome-brand">
    <Logo size={56} />
    <h1>Mirrorleaf is locked</h1>
    <p>{how}</p>
  </div>
  <div class="welcome-choices">
    <button class="primary" disabled={asking} onclick={unlock}>Unlock</button>
  </div>
</main>
