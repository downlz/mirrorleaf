<script lang="ts">
  // After a recovery (E2, assessment §8.4): the account's devices as the
  // ledger knows them, so the lost ones can be removed, then a new kit,
  // because this one was used.
  import { api, type PickedBackup } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';
  import KitStep from './KitStep.svelte';
  import RestoreBackup from './RestoreBackup.svelte';

  let { ondone }: { ondone: () => void } = $props();

  let busy = $state(false);
  let kitStep = $state(false);
  /** A backup picked to restore (E9: kit plus latest backup is a full restore). */
  let picked = $state<PickedBackup | null>(null);

  async function pickBackup() {
    picked = (await app.run(api.pickBackup)) ?? null;
  }

  const others = $derived(app.status?.devices.filter((d) => !d.this_device) ?? []);

  async function remove(id: string) {
    busy = true;
    await app.run(() => api.removeDevice(id));
    await app.loadStatus();
    busy = false;
  }

  async function newKit() {
    busy = true;
    const replaced = (await app.run(() => api.replaceKey([]))) !== undefined;
    busy = false;
    if (replaced) kitStep = true;
  }
</script>

{#if kitStep}
  <KitStep {ondone} />
{:else if picked}
  <RestoreBackup {picked} onclose={() => (picked = null)} />
{:else}
  <h1 class="step-title">Your account is back</h1>
  {#if others.length > 0}
    <p class="hint">
      Your notes come back from any of these that still has them. Remove the ones you lost: they
      can't sync any more.
    </p>
    <ul class="devices">
      {#each others as device (device.id)}
        <li>
          <Icon name="device" size={18} />
          <span class="grow">
            {device.name}
            <small
              >{device.connection ? `connected, ${device.connection}` : 'not connected yet'}</small
            >
          </span>
          <button disabled={busy} onclick={() => remove(device.id)}>Remove</button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="hint">None of your other devices is left on the account.</p>
  {/if}
  <p class="hint">
    Have a backup? Restore it to bring back anything your devices don’t have.
    <button class="link" disabled={busy} onclick={pickBackup}>Restore from a backup…</button>
  </p>
  <p class="hint">Your recovery kit was used, so make a new one. The old one then stops working.</p>
  <div class="actions">
    <button class="primary" disabled={busy} onclick={newKit}>Make a new recovery kit</button>
    <button disabled={busy} onclick={ondone}>Later</button>
  </div>
{/if}
