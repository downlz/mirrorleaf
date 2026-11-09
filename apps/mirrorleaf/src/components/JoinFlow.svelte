<script lang="ts">
  // Adding this device to an account (E1, direction A): scan the other
  // device's QR code (Android) or type its words, check the number both show,
  // and wait for the other device to allow it.
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';
  import Number from './Number.svelte';
  import PermissionsStep from './PermissionsStep.svelte';

  let step = $state<'checking' | 'permissions' | 'code'>('checking');
  let code = $state('');
  let ledger = $state('');
  let showLedger = $state(false);
  let name = $state(app.status?.device_name ?? '');
  let editingName = $state(false);
  let joining = $state(false);
  let error = $state('');

  const android = $derived(app.platform === 'android');
  const status = $derived(app.pairing);

  onMount(async () => {
    if (!android) {
      step = 'code';
      return;
    }
    // Asked once, here, where sync first needs them (direction A).
    const granted = await app.run(api.permissions);
    step = granted?.notifications && granted.nearby ? 'code' : 'permissions';
  });

  const text = (e: unknown) => (e instanceof Error ? e.message : String(e));

  async function scan() {
    error = '';
    try {
      const scanned = await api.scanCode();
      if (scanned) {
        code = scanned;
        await connect();
      }
    } catch (e) {
      error = text(e);
    }
  }

  async function connect() {
    if (!code.trim() || joining) return;
    error = '';
    joining = true;
    try {
      if (name.trim() && name.trim() !== app.status?.device_name) {
        await api.setDeviceName(name.trim());
      }
      await api.joinWithCode(code.trim(), ledger.trim() || null);
      // Joined: the welcome page gives way to the stream.
      app.closeWelcomeSheet();
      app.notify('Joined your account. Your notes are on their way.');
      await Promise.all([app.loadStatus(), app.refresh()]);
    } catch (e) {
      error = text(e);
      if (/ledger/i.test(error)) showLedger = true;
    } finally {
      joining = false;
    }
  }
</script>

{#if step === 'permissions'}
  <PermissionsStep ondone={() => (step = 'code')} />
{:else if joining && status.state === 'confirm'}
  <div class="confirm">
    <h1 class="step-title">Check this number</h1>
    <Number value={status.number} />
    <p class="hint">Your other device shows the same number. Allow this {app.deviceWord} there.</p>
    <p class="waiting"><span class="spinner"></span>Waiting for your other device</p>
  </div>
{:else if joining}
  <p class="waiting"><span class="spinner"></span>Connecting to your other device…</p>
{:else if step === 'code'}
  <p class="hint">
    On your other device, open <b>Settings → Devices → Add a device</b>.
  </p>
  {#if android}
    <button class="primary wide" onclick={scan}><Icon name="camera" /> Scan the code</button>
  {/if}
  <label class="field">
    <span
      >{android ? 'Or type the words it shows' : 'Type the words it shows, or paste its code'}</span
    >
    <input
      id="pair-code"
      bind:value={code}
      autocapitalize="none"
      autocomplete="off"
      spellcheck="false"
      placeholder="7-acoustic-planet-river"
      onkeydown={(e) => e.key === 'Enter' && connect()}
    />
  </label>
  {#if showLedger}
    <label class="field">
      <span>Ledger address, to find your other device by its words</span>
      <input
        id="pair-ledger"
        type="url"
        bind:value={ledger}
        placeholder="http://192.168.1.20:8080"
      />
    </label>
  {/if}
  {#if editingName}
    <label class="field">
      <span>Name this {app.deviceWord}</span>
      <input id="pair-name" bind:value={name} />
    </label>
  {:else}
    <p class="hint">
      Your other devices will call it “{name}”.
      <button class="link" onclick={() => (editingName = true)}>Change</button>
    </p>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button class:primary={!android} disabled={!code.trim()} onclick={connect}>Connect</button>
    {#if !showLedger}
      <button class="link" onclick={() => (showLedger = true)}>Ledger…</button>
    {/if}
  </div>
{/if}
