<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type Permissions } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import AddDevice from './AddDevice.svelte';
  import Icon from './Icon.svelte';
  import SyncStatus from './SyncStatus.svelte';

  let ledgerUrl = $state('');
  let note = $state('');
  let busy = $state(false);
  let autostart = $state<boolean | null>(null);
  /** Android's sync permissions (E1): asked when adding a device, changeable here. */
  let permissions = $state<Permissions | null>(null);
  /** Settings itself, or adding a device (E1, direction A). */
  let view = $state<'settings' | 'add'>('settings');

  const status = $derived(app.status);

  onMount(async () => {
    ledgerUrl = app.status?.ledger_url ?? '';
    if (app.isDesktop) autostart = (await app.run(api.autostart)) ?? null;
    if (app.platform === 'android') permissions = (await app.run(api.permissions)) ?? null;
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

  async function allow(which: 'notifications' | 'nearby') {
    permissions = (await app.run(() => api.requestPermissions([which]))) ?? permissions;
  }

  async function setAutostart(enabled: boolean) {
    await app.run(() => api.setAutostart(enabled));
    autostart = (await app.run(api.autostart)) ?? autostart;
  }

  async function leaveAdding() {
    view = 'settings';
    await app.loadStatus();
  }

  function keydown(event: KeyboardEvent) {
    if (event.key !== 'Escape') return;
    if (view === 'add') void leaveAdding();
    else app.closeSettings();
  }
</script>

<svelte:window onkeydown={keydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onpointerdown={() => app.closeSettings()}></div>
<div
  class="sheet"
  role="dialog"
  aria-modal="true"
  aria-label={view === 'add' ? 'Add a device' : 'Settings'}
>
  <header class="sheet-header">
    {#if view === 'add'}
      <button class="icon-button" title="Back to Settings" onclick={leaveAdding}>
        <Icon name="back" />
      </button>
    {/if}
    <h1 class="grow">{view === 'add' ? 'Add a device' : 'Settings'}</h1>
    <button class="icon-button" title="Close" onclick={() => app.closeSettings()}>
      <Icon name="close" />
    </button>
  </header>

  <div class="sheet-body">
    {#if view === 'add'}
      <section><AddDevice /></section>
    {:else if status}
      <section>
        <h2>Sync</h2>
        <SyncStatus />
        {#if status.account}
          <p class="hint">Account {status.account.slice(0, 8)}</p>
          <div class="actions">
            <button onclick={() => (view = 'add')}>Add a device…</button>
          </div>

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

      {#if permissions && (!permissions.notifications || !permissions.nearby)}
        <section>
          <h2>This {app.deviceWord}</h2>
          {#if !permissions.notifications}
            <div class="permission-row">
              <span class="grow">
                <b>Notifications are off</b>
                <small>Your other devices can't wake this {app.deviceWord} to sync.</small>
              </span>
              <button onclick={() => allow('notifications')}>Allow</button>
            </div>
          {/if}
          {#if !permissions.nearby}
            <div class="permission-row">
              <span class="grow">
                <b>Nearby devices are off</b>
                <small>Sync goes over the internet, even on the same Wi-Fi.</small>
              </span>
              <button onclick={() => allow('nearby')}>Allow</button>
            </div>
          {/if}
        </section>
      {/if}

      {#if status.account}
        <section>
          <h2>Ledger</h2>
          <p class="hint">
            Tells your devices which of them is behind, and wakes your phone. It never sees your
            notes.
          </p>
          <label class="field">
            <span>Address</span>
            <input
              id="ledger-url"
              type="url"
              bind:value={ledgerUrl}
              placeholder="http://192.168.1.20:8080"
            />
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
