<script lang="ts">
  import { app } from '../lib/state.svelte';

  // Honest sync status (assessment §5): who this device is connected to now.
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

<button class="sync-status" title="Sync settings" onclick={() => app.openSettings()}>
  <span class="dot {summary.tone}"></span>
  <span class="sync-text">{summary.text}</span>
</button>
