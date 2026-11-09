<script lang="ts">
  // Android's two sync permissions, explained before Android asks (E1): at the
  // moment they're needed, not at every start.
  import { api } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';

  let { ondone }: { ondone: () => void } = $props();
  let busy = $state(false);

  async function ask() {
    busy = true;
    await app.run(() => api.requestPermissions(['notifications', 'nearby']));
    busy = false;
    ondone();
  }
</script>

<h1 class="step-title">Two permissions</h1>
<ul class="permission-list">
  <li>
    <span class="permission-icon"><Icon name="bell" /></span>
    <span>
      <b>Notifications</b>
      So your other devices can wake this {app.deviceWord} to sync, and tell you when one of them needs
      opening.
    </span>
  </li>
  <li>
    <span class="permission-icon"><Icon name="nearby" /></span>
    <span>
      <b>Nearby devices</b>
      Faster sync with your devices on the same Wi-Fi. Without it, sync goes over the internet.
    </span>
  </li>
</ul>
<div class="actions">
  <button class="primary" disabled={busy} onclick={ask}>Continue</button>
  <button disabled={busy} onclick={ondone}>Not now</button>
</div>
<p class="hint">Android asks for each next. You can change them later in Settings.</p>
