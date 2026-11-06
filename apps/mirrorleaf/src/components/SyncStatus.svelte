<script lang="ts">
  import { app } from '../lib/state.svelte';

  // Honest sync status (assessment §5): who this device is connected to now.
  // Shown in Settings, not on the main screen (owner, 6 Nov 2026): the main
  // screen stays clean and only speaks up when the user has to act (the
  // fallback banner, or a dot on the Settings button while sync isn't set up).
  const summary = $derived.by(() => {
    const status = app.status;
    if (!status) return { tone: 'idle', text: 'Starting…' };
    if (!status.account) return { tone: 'warn', text: 'Sync not set up' };
    if (!status.live) return { tone: 'idle', text: 'Sync paused' };
    const connected = status.devices.filter((d) => d.connection);
    if (connected.length === 0) return { tone: 'idle', text: 'Waiting for your devices' };
    const names = connected.map((d) => d.name).join(', ');
    return { tone: 'ok', text: `Syncing with ${names}` };
  });
</script>

<p class="sync-status" role="status">
  <span class="dot {summary.tone}"></span>
  <span>{summary.text}</span>
</p>
