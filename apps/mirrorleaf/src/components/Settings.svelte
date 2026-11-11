<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api,
    type DeviceSummary,
    type LockSettings,
    type Permissions,
    type PickedBackup,
  } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import AddDevice from './AddDevice.svelte';
  import Icon from './Icon.svelte';
  import RestoreBackup from './RestoreBackup.svelte';
  import SyncStatus from './SyncStatus.svelte';

  let ledgerUrl = $state('');
  let note = $state('');
  let busy = $state(false);
  let autostart = $state<boolean | null>(null);
  /** Android's sync permissions (E1): asked when adding a device, changeable here. */
  let permissions = $state<Permissions | null>(null);
  /** Settings itself, or one of its steps: adding a device (E1), removing one,
   *  a new recovery kit, signing out (M10), restoring a backup (M11). */
  let view = $state<'settings' | 'add' | 'remove' | 'kit' | 'signout' | 'restore'>('settings');
  /** The backup picked to restore. */
  let picked = $state<PickedBackup | null>(null);
  /** The app lock (E4). */
  let lockSettings = $state<LockSettings | null>(null);

  const LOCK_AFTER = [
    { minutes: 0, label: 'Immediately' },
    { minutes: 1, label: 'After 1 minute away' },
    { minutes: 5, label: 'After 5 minutes away' },
    { minutes: 15, label: 'After 15 minutes away' },
  ];
  /** The device being removed. */
  let target = $state<DeviceSummary | null>(null);
  /** Changes only this device holds, for signing out. */
  let onlyHere = $state<number | null>(null);
  let working = $state(false);

  const titles = {
    settings: 'Settings',
    add: 'Add a device',
    remove: 'Remove a device',
    kit: 'New recovery kit',
    signout: 'Sign out',
    restore: 'Restore a backup',
  };

  const longDate = (seconds: number) =>
    new Date(seconds * 1000).toLocaleDateString('en-GB', {
      day: 'numeric',
      month: 'long',
      year: 'numeric',
    });

  function askRemove(device: DeviceSummary) {
    target = device;
    view = 'remove';
  }

  async function remove(replaceKey: boolean) {
    if (!target) return;
    working = true;
    const id = target.id;
    const done = await app.run(() => (replaceKey ? api.replaceKey([id]) : api.removeDevice(id)));
    working = false;
    if (done === undefined) return;
    app.notify(
      replaceKey
        ? `Removed ${target.name}. Save your new recovery kit.`
        : `Removed ${target.name}.`,
    );
    target = null;
    view = 'settings';
    await app.loadStatus();
  }

  async function newKit() {
    working = true;
    const done = await app.run(() => api.replaceKey([]));
    working = false;
    if (done === undefined) return;
    view = 'settings';
    // The kit shows over Settings until it is saved.
    await app.loadStatus();
  }

  async function askSignOut() {
    view = 'signout';
    onlyHere = (await app.run(api.onlyHere)) ?? null;
  }

  async function signOut() {
    working = true;
    // The app starts afresh once this device is erased.
    await app.run(() => api.signOut((onlyHere ?? 0) > 0));
    working = false;
  }

  const status = $derived(app.status);

  async function pickBackup() {
    const backup = await app.run(api.pickBackup);
    if (!backup) return;
    picked = backup;
    view = 'restore';
  }

  function closeRestore() {
    picked = null;
    view = 'settings';
  }

  async function setEncrypt(on: boolean) {
    await app.run(() => api.setBackupEncryption(on));
    await app.loadStatus();
  }

  /** Any change to the lock asks the device's own unlock first. */
  async function setLock(
    control: HTMLInputElement | HTMLSelectElement,
    enabled: boolean,
    minutes: number,
  ) {
    working = true;
    await app.run(() => api.setLock(enabled, minutes));
    working = false;
    // Shows what holds now, also when the unlock was cancelled: the control
    // itself is put back, as an unchanged setting doesn't redraw it.
    lockSettings = (await app.run(api.lockSettings)) ?? lockSettings;
    if (!lockSettings) return;
    if (control instanceof HTMLInputElement) control.checked = lockSettings.enabled;
    else control.value = String(lockSettings.after_minutes);
  }

  onMount(async () => {
    ledgerUrl = app.status?.ledger_url ?? '';
    if (app.status?.account) lockSettings = (await app.run(api.lockSettings)) ?? null;
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
    if (event.key !== 'Escape' || working) return;
    if (view === 'add') void leaveAdding();
    else if (view === 'restore' && picked) {
      void api.forgetBackup(picked.path).catch(() => {});
      closeRestore();
    } else if (view !== 'settings') view = 'settings';
    else app.closeSettings();
  }
</script>

<svelte:window onkeydown={keydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onpointerdown={() => app.closeSettings()}></div>
<div class="sheet" role="dialog" aria-modal="true" aria-label={titles[view]}>
  <header class="sheet-header">
    {#if view !== 'settings'}
      <button
        class="icon-button"
        title="Back to Settings"
        disabled={working}
        onclick={() =>
          view === 'add'
            ? leaveAdding()
            : view === 'restore' && picked
              ? (void api.forgetBackup(picked.path).catch(() => {}), closeRestore())
              : (view = 'settings')}
      >
        <Icon name="back" />
      </button>
    {/if}
    <h1 class="grow">{titles[view]}</h1>
    <button class="icon-button" title="Close" onclick={() => app.closeSettings()}>
      <Icon name="close" />
    </button>
  </header>

  <div class="sheet-body">
    {#if view === 'add'}
      <section><AddDevice /></section>
    {:else if view === 'restore' && picked}
      <section><RestoreBackup {picked} onclose={closeRestore} /></section>
    {:else if view === 'remove' && target}
      <section>
        <h2 class="question">Remove “{target.name}”?</h2>
        <p class="hint">
          It stops syncing with your devices, for good. To use it again, erase it and add it again.
        </p>
        <div class="actions wrap">
          <button class="danger" disabled={working} onclick={() => remove(false)}>Remove</button>
          <button disabled={working} onclick={() => (view = 'settings')}>Cancel</button>
        </div>
        <h3>Lost or stolen?</h3>
        <p class="hint">
          It still holds your account key. Remove it and make a new recovery kit: your other devices
          get a new key, and your old kit stops working.
        </p>
        <div class="actions">
          <button disabled={working} onclick={() => remove(true)}
            >Remove and make a new recovery kit</button
          >
        </div>
      </section>
    {:else if view === 'kit'}
      <section>
        <p class="hint">
          A new recovery kit comes with a new account key. Your other devices get the key the next
          time they connect to this {app.deviceWord}, or to another device that has it. Your old kit
          stops working.
        </p>
        <div class="actions">
          <button class="primary" disabled={working} onclick={newKit}>Make the new kit</button>
          <button disabled={working} onclick={() => (view = 'settings')}>Cancel</button>
        </div>
      </section>
    {:else if view === 'signout'}
      <section>
        <p class="hint">
          This {app.deviceWord} leaves your account, and its notes and photos are erased from it. Your
          other devices keep everything.
        </p>
        {#if onlyHere === null}
          <p class="waiting">
            <span class="spinner"></span>Checking what only this {app.deviceWord} has…
          </p>
        {:else}
          {#if onlyHere > 0}
            <p class="error" role="alert">
              {onlyHere === 1 ? '1 change exists' : `${onlyHere} changes exist`} only on this
              {app.deviceWord}. Let it sync with another device first, or they are lost.
            </p>
          {/if}
          <div class="actions">
            <button class="danger" disabled={working} onclick={signOut}
              >{onlyHere > 0 ? 'Sign out anyway' : 'Sign out and erase'}</button
            >
            <button disabled={working} onclick={() => (view = 'settings')}>Cancel</button>
          </div>
        {/if}
      </section>
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
                {#if !device.this_device}
                  <button class="link" onclick={() => askRemove(device)}>Remove…</button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        {#if note}<p class="result">{note}</p>{/if}
      </section>

      {#if status.account}
        <section>
          <h2>Recovery kit</h2>
          {#if status.kit_used}
            <p class="error">
              Your kit was used to recover this account. Make a new one, so the old one stops
              working.
            </p>
          {:else if status.kit_made_at}
            <p class="hint">
              Made {longDate(status.kit_made_at)}. It brings your account back if you lose all your
              devices.
            </p>
          {:else}
            <p class="error">
              Your account has no recovery kit. If you lose all your devices, your notes are gone.
            </p>
          {/if}
          <div class="actions">
            <button onclick={() => (view = 'kit')}>Make a new recovery kit…</button>
          </div>
        </section>
      {/if}

      {#if status.account && app.backup}
        <section>
          <h2>Backups</h2>
          <p class="hint">
            {app.backup.last_backup ? `Last backup ${app.backup.last_backup}.` : 'No backup yet.'}
            A backup holds all your notes and photos. Keep it somewhere other than your devices.
          </p>
          <label class="toggle">
            <input
              id="backup-encrypt"
              type="checkbox"
              checked={app.backup.encrypt}
              onchange={(e) => setEncrypt(e.currentTarget.checked)}
            />
            <span>Encrypt backups</span>
          </label>
          {#if app.backup.encrypt}
            <p class="hint">
              Encrypted backups open only on your devices, or with the recovery kit you have when
              you make them.
            </p>
          {:else}
            <p class="hint">
              This file isn't encrypted: keep it somewhere private, or turn on Encrypt backups.
            </p>
          {/if}
          <div class="actions wrap">
            <button onclick={() => app.backUpNow()}>Back up now…</button>
            <button onclick={pickBackup}>Restore from a backup…</button>
          </div>
        </section>
      {/if}

      {#if status.account && lockSettings}
        <section>
          <h2>App lock</h2>
          <label class="toggle">
            <input
              id="app-lock"
              type="checkbox"
              checked={lockSettings.enabled}
              disabled={working || (!lockSettings.available && !lockSettings.enabled)}
              onchange={(e) =>
                setLock(e.currentTarget, e.currentTarget.checked, lockSettings?.after_minutes ?? 5)}
            />
            <span>Lock Mirrorleaf</span>
          </label>
          {#if !lockSettings.available}
            <p class="error">
              {app.platform === 'android'
                ? 'Set a screen lock on this phone first.'
                : app.platform === 'macos'
                  ? 'Set a login password on this Mac first.'
                  : 'The app lock isn’t available on this computer.'}
            </p>
          {:else}
            <p class="hint">
              {app.platform === 'android'
                ? 'Unlock with your fingerprint, face or screen lock.'
                : 'Unlock with Touch ID or your Mac’s password.'}
              Your notes keep syncing while it’s locked.
            </p>
          {/if}
          {#if lockSettings.enabled}
            <label class="field">
              <span>Lock</span>
              <select
                id="lock-after"
                value={lockSettings.after_minutes}
                disabled={working}
                onchange={(e) => setLock(e.currentTarget, true, Number(e.currentTarget.value))}
              >
                {#each LOCK_AFTER as choice (choice.minutes)}
                  <option value={choice.minutes}>{choice.label}</option>
                {/each}
              </select>
            </label>
          {/if}
        </section>
      {/if}

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

      {#if status.account}
        <section>
          <h2>Sign out</h2>
          <div class="actions">
            <button class="danger-text" onclick={askSignOut}
              >Sign out of this {app.deviceWord}…</button
            >
          </div>
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
