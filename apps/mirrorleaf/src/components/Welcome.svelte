<script lang="ts">
  // First run (E1, direction A "Three doors", owner, 9 Nov 2026): one calm page
  // with the assessment's three choices (§8.1). No email, no password. Starting
  // an account shows its recovery kit before anything else (E2, M10).
  import { api } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';
  import JoinFlow from './JoinFlow.svelte';
  import KitStep from './KitStep.svelte';
  import Logo from './Logo.svelte';
  import PermissionsStep from './PermissionsStep.svelte';
  import RecoverFlow from './RecoverFlow.svelte';

  let name = $state(app.status?.device_name ?? '');
  let busy = $state(false);
  let error = $state('');
  /** Starting an account: the name, then the kit, then (Android) the permissions. */
  let step = $state<'name' | 'kit' | 'permissions'>('name');

  const titles = {
    create: 'Start a new account',
    join: `Add this ${app.deviceWord}`,
    recover: 'Recover my account',
  };

  async function start() {
    busy = true;
    error = '';
    try {
      if (name.trim() && name.trim() !== app.status?.device_name) {
        await api.setDeviceName(name.trim());
      }
      app.onboarding = true;
      await api.createAccount();
      step = 'kit';
    } catch (e) {
      app.onboarding = false;
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function kitSaved() {
    if (app.platform === 'android') {
      const granted = await app.run(api.permissions);
      if (!granted?.notifications || !granted.nearby) {
        step = 'permissions';
        return;
      }
    }
    await finish('Your account is ready. Add your other devices from Settings.');
  }

  async function finish(message: string | null = null) {
    app.onboarding = false;
    app.closeWelcomeSheet();
    await Promise.all([app.loadStatus(), app.refresh()]);
    if (message) app.notify(message);
  }

  /** Closing the sheet once the account exists ends setting up; a kit not
   *  saved yet comes back over the notes. */
  function close() {
    if (busy) return;
    if (app.onboarding) void finish();
    else app.closeWelcomeSheet();
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && app.welcomeSheet) close();
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
    <button class="link" onclick={() => app.openWelcomeSheet('recover')}>Recover my account</button>
  </div>
</main>

{#if app.welcomeSheet}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="scrim" onpointerdown={close}></div>
  <div
    class="sheet welcome-sheet"
    role="dialog"
    aria-modal="true"
    aria-label={titles[app.welcomeSheet]}
  >
    <header class="sheet-header">
      <h1 class="grow">{titles[app.welcomeSheet]}</h1>
      <button class="icon-button" title="Close" onclick={close}>
        <Icon name="close" />
      </button>
    </header>
    <div class="sheet-body">
      {#if app.welcomeSheet === 'join'}
        <JoinFlow />
      {:else if app.welcomeSheet === 'recover'}
        <RecoverFlow ondone={() => finish('Welcome back. Your notes are syncing.')} />
      {:else if step === 'kit'}
        <KitStep ondone={kitSaved} />
      {:else if step === 'permissions'}
        <PermissionsStep
          ondone={() => finish('Your account is ready. Add your other devices from Settings.')}
        />
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
