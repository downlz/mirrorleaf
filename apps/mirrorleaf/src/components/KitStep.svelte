<script lang="ts">
  // The recovery kit (E2, assessment §8.4): 12 words that bring the account
  // back if every device is lost. Shown once, then saved as a PDF or as text,
  // or written down, before setup goes on (§8.1: the step can't be skipped).
  import { onMount } from 'svelte';
  import { api, type KitView } from '../lib/api';
  import { app } from '../lib/state.svelte';

  let { ondone }: { ondone: () => void } = $props();

  let kit = $state<KitView | null>(null);
  let saved = $state<('pdf' | 'text')[]>([]);
  let wrote = $state(false);
  let busy = $state(false);
  let error = $state('');

  const ready = $derived(saved.length > 0 || wrote);

  onMount(async () => {
    kit = (await app.run(api.pendingKit)) ?? null;
    if (!kit) ondone();
  });

  async function save(kind: 'pdf' | 'text') {
    error = '';
    try {
      if (await api.saveKit(kind)) saved = [...saved, kind];
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function done() {
    busy = true;
    await app.run(api.finishKit);
    busy = false;
    ondone();
  }
</script>

{#if kit}
  <h1 class="step-title">Your recovery kit</h1>
  <p class="hint">
    If you lose all your devices, these 12 words bring your account back. Anyone who has them can
    open your account, so keep them offline: printed, in a password manager or on a USB drive.
  </p>
  <ol class="kit-words" aria-label="Your 12 recovery words">
    {#each kit.words as word, i (i)}
      <li><span>{i + 1}</span><b>{word}</b></li>
    {/each}
  </ol>
  <p class="meta">Account {kit.account} · made {kit.made} · shown only now</p>
  <div class="actions wrap">
    <button class:primary={!ready} onclick={() => save('pdf')}>
      {saved.includes('pdf') ? 'PDF saved' : 'Save the PDF'}
    </button>
    <button onclick={() => save('text')}>
      {saved.includes('text') ? 'Text saved' : 'Save as text'}
    </button>
  </div>
  <label class="toggle">
    <input id="kit-wrote" type="checkbox" bind:checked={wrote} />
    <span>I've written the words down</span>
  </label>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button class:primary={ready} disabled={busy || !ready} onclick={done}>Done</button>
  </div>
{:else}
  <p class="waiting"><span class="spinner"></span>Making your recovery kit…</p>
{/if}
