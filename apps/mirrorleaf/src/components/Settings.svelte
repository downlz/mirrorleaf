<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';

  // Phase 0's developer account (plan §5.4). Real pairing with QR codes, the
  // recovery kit and app lock replace this in Phase 1 (E1–E4).
  let ticketIn = $state('');
  let ticketOut = $state('');
  let ledgerUrl = $state('');
  let note = $state('');
  let busy = $state(false);
  let autostart = $state<boolean | null>(null);

  const status = $derived(app.status);

  onMount(async () => {
    ledgerUrl = app.status?.ledger_url ?? '';
    if (app.isDesktop) autostart = (await app.run(api.autostart)) ?? null;
  });

  async function act(action: () => Promise<string>) {
    busy = true;
    note = '';
    const result = await app.run(action);
    busy = false;
    if (result) note = result;
    await app.loadStatus();
    await app.refresh();
  }

  async function showTicket() {
    busy = true;
    ticketOut = (await app.run(api.ticket)) ?? '';
    busy = false;
  }

  async function copyTicket(event: MouseEvent) {
    const field = (event.currentTarget as HTMLElement)
      .previousElementSibling as HTMLTextAreaElement;
    field.select();
    try {
      await navigator.clipboard.writeText(ticketOut);
      note = 'Ticket copied.';
    } catch {
      note = 'Selected: press ⌘C to copy.';
    }
  }

  async function setAutostart(enabled: boolean) {
    await app.run(() => api.setAutostart(enabled));
    autostart = (await app.run(api.autostart)) ?? autostart;
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === 'Escape') app.closeSettings();
  }
</script>

<svelte:window onkeydown={keydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onpointerdown={() => app.closeSettings()}></div>
<div class="sheet" role="dialog" aria-modal="true" aria-label="Settings">
  <header class="sheet-header">
    <h1 class="grow">Settings</h1>
    <button class="icon-button" title="Close" onclick={() => app.closeSettings()}>
      <Icon name="close" />
    </button>
  </header>

  <div class="sheet-body">
    {#if status}
      <section>
        <h2>Sync</h2>
        {#if !status.account}
          <p class="hint">
            Notes sync only between your own devices. Start an account on your first device, then
            join it from the others with a ticket.
          </p>
          <div class="actions">
            <button class="primary" disabled={busy} onclick={() => act(api.createAccount)}
              >Start a new account</button
            >
          </div>
          <label class="field">
            <span>Join with a ticket from another device</span>
            <textarea rows="3" bind:value={ticketIn} placeholder="mlt1…"></textarea>
          </label>
          <div class="actions">
            <button
              disabled={busy || !ticketIn.trim()}
              onclick={() => act(() => api.join(ticketIn))}>Join</button
            >
          </div>
        {:else}
          <p class="hint">Account {status.account.slice(0, 8)}</p>
          {#if ticketOut}
            <label class="field">
              <span
                >Ticket for a new device. It holds the account key: share it only with your own
                devices.</span
              >
              <textarea rows="4" readonly value={ticketOut}></textarea>
              <button onclick={copyTicket}>Copy</button>
            </label>
          {:else}
            <div class="actions">
              <button disabled={busy} onclick={showTicket}>Add a device…</button>
            </div>
          {/if}

          <h3>Devices</h3>
          <ul class="devices">
            {#each status.devices as device (device.id)}
              <li>
                <Icon name="device" size={18} />
                <span class="grow">
                  {device.name}{device.this_device ? ' (this device)' : ''}
                  <small>
                    {#if device.this_device}
                      {status.live ? 'syncing' : 'sync paused'}
                    {:else if device.connection}
                      connected, {device.connection}
                    {:else if device.online !== null}
                      {device.online ? 'online' : 'offline'}{device.behind
                        ? `, ${device.behind} changes behind`
                        : ''}
                    {:else}
                      not connected
                    {/if}
                  </small>
                </span>
              </li>
            {/each}
          </ul>
        {/if}
        {#if note}<p class="result">{note}</p>{/if}
      </section>

      {#if status.account}
        <section>
          <h2>Ledger</h2>
          <p class="hint">
            Tells your devices which of them is behind, and wakes your phone. It never sees your
            notes.
          </p>
          <label class="field">
            <span>Address</span>
            <input type="url" bind:value={ledgerUrl} placeholder="http://192.168.1.20:8080" />
          </label>
          <div class="actions">
            <button
              disabled={busy || !ledgerUrl.trim()}
              onclick={() => act(() => api.useLedger(ledgerUrl))}>Use this ledger</button
            >
          </div>
        </section>
      {/if}

      {#if app.isDesktop && autostart !== null}
        <section>
          <h2>This computer</h2>
          <label class="toggle">
            <input
              type="checkbox"
              checked={autostart}
              onchange={(e) => setAutostart(e.currentTarget.checked)}
            />
            <span>Open at login and keep syncing in the menu bar</span>
          </label>
          <p class="hint">
            Closing the window leaves Mirrorleaf syncing in the menu bar. Choose Quit from its menu
            to stop it.
          </p>
        </section>
      {/if}

      <section>
        <h2>About</h2>
        <p class="hint">
          {status.device_name} · device {status.device_id.slice(0, 8)} · {status.notes} notes ·
          {status.changes} changes<br />
          Mirrorleaf Phase 0. Your notes, mirrored on your devices. Nowhere else.
        </p>
      </section>
    {/if}
  </div>
</div>
