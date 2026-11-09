<script lang="ts">
  // Recovering the account with its kit (E2, assessment §8.4): scan the
  // kit's QR code (Android) or type its 12 words. The notes come back from
  // the account's other devices; then the lost ones are removed and a new kit
  // made.
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';
  import PermissionsStep from './PermissionsStep.svelte';
  import RecoveredView from './RecoveredView.svelte';

  let { ondone }: { ondone: () => void } = $props();

  let step = $state<'checking' | 'permissions' | 'code' | 'back'>('checking');
  let code = $state('');
  let ledger = $state('');
  let showLedger = $state(false);
  let name = $state(app.status?.device_name ?? '');
  let editingName = $state(false);
  let working = $state(false);
  let error = $state('');

  const android = $derived(app.platform === 'android');

  onMount(async () => {
    if (!android) {
      step = 'code';
      return;
    }
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
        await recover();
      }
    } catch (e) {
      error = text(e);
    }
  }

  async function recover() {
    if (!code.trim() || working) return;
    error = '';
    working = true;
    app.onboarding = true;
    try {
      if (name.trim() && name.trim() !== app.status?.device_name) {
        await api.setDeviceName(name.trim());
      }
      const said = await api.recover(code.trim(), ledger.trim() || null);
      app.notify(said);
      await app.loadStatus();
      step = 'back';
    } catch (e) {
      app.onboarding = false;
      error = text(e);
      if (/ledger/i.test(error)) showLedger = true;
    } finally {
      working = false;
    }
  }
</script>

{#if step === 'permissions'}
  <PermissionsStep ondone={() => (step = 'code')} />
{:else if step === 'back'}
  <RecoveredView {ondone} />
{:else if working}
  <p class="waiting"><span class="spinner"></span>Finding your account and your other devices…</p>
{:else if step === 'code'}
  <p class="hint">Use the recovery kit you saved when you set up Mirrorleaf, or your newest one.</p>
  {#if android}
    <button class="primary wide" onclick={scan}><Icon name="camera" /> Scan the kit's code</button>
  {/if}
  <label class="field">
    <span
      >{android
        ? 'Or type the 12 words'
        : 'Type the 12 words, or paste the code from the kit'}</span
    >
    <textarea
      id="recover-code"
      rows="3"
      bind:value={code}
      autocapitalize="none"
      autocomplete="off"
      spellcheck="false"
      placeholder="The 12 words, in order"></textarea>
  </label>
  {#if showLedger}
    <label class="field">
      <span>Ledger address, to find your other devices</span>
      <input
        id="recover-ledger"
        type="url"
        bind:value={ledger}
        placeholder="http://192.168.1.20:8080"
      />
    </label>
  {/if}
  {#if editingName}
    <label class="field">
      <span>Name this {app.deviceWord}</span>
      <input id="recover-name" bind:value={name} />
    </label>
  {:else}
    <p class="hint">
      Your other devices will call it “{name}”.
      <button class="link" onclick={() => (editingName = true)}>Change</button>
    </p>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button class:primary={!android} disabled={!code.trim()} onclick={recover}>Recover</button>
    {#if !showLedger}
      <button class="link" onclick={() => (showLedger = true)}>Ledger…</button>
    {/if}
  </div>
{/if}
