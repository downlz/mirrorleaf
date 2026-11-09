<script lang="ts">
  // First run (E1, direction A "Three doors", owner, 9 Nov 2026): one calm page
  // with the assessment's three choices (§8.1). No email, no password.
  import { api } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';
  import JoinFlow from './JoinFlow.svelte';
  import Logo from './Logo.svelte';
  import PermissionsStep from './PermissionsStep.svelte';

  let name = $state(app.status?.device_name ?? '');
  let busy = $state(false);
  let error = $state('');
  /** After starting an account on Android: the permissions, before the stream. */
  let askPermissions = $state(false);

  async function start() {
    busy = true;
    error = '';
    try {
      if (name.trim() && name.trim() !== app.status?.device_name) {
        await api.setDeviceName(name.trim());
      }
      await api.createAccount();
      if (app.platform === 'android') {
        const granted = await app.run(api.permissions);
        if (!granted?.notifications || !granted.nearby) {
          askPermissions = true;
          return;
        }
      }
      await finish();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function finish() {
    app.closeWelcomeSheet();
    await Promise.all([app.loadStatus(), app.refresh()]);
    app.notify('Your account is ready. Add your other devices from Settings.');
  }

  function recover() {
    app.notify('Recovering an account comes with the recovery kit, in the next update.');
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && app.welcomeSheet) app.closeWelcomeSheet();
  }
</script>

<svelte:window onkeydown={keydown} />

<main class="welcome" data-tauri-drag-region>
  <div class="welcome-brand">
    <Logo size={72} />
    <h1>Mirrorleaf</h1>
    <p>Your notes, mirrored on your devices. Nowhere else.</p>
  </div>
  <div class="welcome-choices">
    <button class="primary" onclick={() => app.openWelcomeSheet('create')}
      >Start a new account</button
    >
    <button onclick={() => app.openWelcomeSheet('join')}
      >Add this {app.deviceWord} to my account</button
    >
    <button class="link" onclick={recover}>Recover my account</button>
  </div>
</main>

{#if app.welcomeSheet}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="scrim" onpointerdown={() => !busy && app.closeWelcomeSheet()}></div>
  <div
    class="sheet welcome-sheet"
    role="dialog"
    aria-modal="true"
    aria-label={app.welcomeSheet === 'create'
      ? 'Start a new account'
      : `Add this ${app.deviceWord}`}
  >
    <header class="sheet-header">
      <h1 class="grow">
        {app.welcomeSheet === 'create' ? 'Start a new account' : `Add this ${app.deviceWord}`}
      </h1>
      <button class="icon-button" title="Close" onclick={() => app.closeWelcomeSheet()}>
        <Icon name="close" />
      </button>
    </header>
    <div class="sheet-body">
      {#if app.welcomeSheet === 'join'}
        <JoinFlow />
      {:else if askPermissions}
        <PermissionsStep ondone={finish} />
      {:else}
        <p class="hint">
          Your notes stay on your devices. Once you've started, add your other devices from
          Settings.
        </p>
        <label class="field">
          <span>Name this {app.deviceWord}</span>
          <input
            id="account-device-name"
            bind:value={name}
            onkeydown={(e) => e.key === 'Enter' && start()}
          />
        </label>
        {#if error}<p class="error" role="alert">{error}</p>{/if}
        <div class="actions">
          <button class="primary" disabled={busy || !name.trim()} onclick={start}>Start</button>
          <button disabled={busy} onclick={() => app.closeWelcomeSheet()}>Cancel</button>
        </div>
      {/if}
    </div>
  </div>
{/if}
