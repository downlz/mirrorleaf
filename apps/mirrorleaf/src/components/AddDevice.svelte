<script lang="ts">
  // Settings → Devices → Add a device (E1, direction A): a QR code and a word
  // code that work once, for 10 minutes; then "Allow … to join?" with the
  // number both devices show.
  import { onDestroy, onMount } from 'svelte';
  import { api, type Invitation } from '../lib/api';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';
  import Number from './Number.svelte';

  let invitation = $state<Invitation | null>(null);
  let now = $state(Date.now());
  let busy = $state(false);
  let showText = $state(false);
  // The person pressed "Don't allow": the ending is their choice, not an error.
  let refused = $state(false);

  const status = $derived(app.pairing);
  const left = $derived(invitation ? Math.max(0, invitation.expires_ms - now) : 0);

  async function make() {
    invitation = null;
    showText = false;
    refused = false;
    invitation = (await app.run(api.invite)) ?? null;
    await app.loadPairing();
  }

  onMount(() => {
    void make();
    const tick = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(tick);
  });

  // Leaving the view withdraws a code that is still showing.
  onDestroy(() => {
    if (['inviting', 'request'].includes(app.pairing.state)) void api.cancelPairing();
  });

  async function answer(allow: boolean) {
    busy = true;
    refused = !allow;
    await app.run(() => api.answerPairing(allow));
    busy = false;
  }

  const clock = (ms: number) =>
    `${Math.floor(ms / 60000)}:${String(Math.floor(ms / 1000) % 60).padStart(2, '0')}`;
</script>

{#if status.state === 'request'}
  <h2 class="question">Allow “{status.name}” to join?</h2>
  <p class="hint">It will get all your notes and photos. Check it shows this number:</p>
  <Number value={status.number} small />
  <div class="actions">
    <button disabled={busy} onclick={() => answer(false)}>Don't allow</button>
    <button class="primary" disabled={busy} onclick={() => answer(true)}>Allow</button>
  </div>
{:else if status.state === 'joined'}
  <p class="done"><Icon name="check" /> {status.name} joined. Your notes are syncing to it now.</p>
{:else if status.state === 'ended'}
  {#if refused}
    <p class="hint" role="status">{status.reason}</p>
  {:else}
    <p class="error" role="alert">{status.reason}</p>
  {/if}
  <div class="actions"><button onclick={make}>Make a new code</button></div>
{:else if invitation && left > 0}
  <div class="pair-code">
    <!-- The engine draws the QR code (our own SVG, from the qrcode crate). -->
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <div class="qr">{@html invitation.qr_svg}</div>
    <div class="pair-words">
      <p class="hint">
        On the new device, choose <b>Add this device to my account</b>, then scan this code{invitation.words
          ? ' or type these words:'
          : '.'}
      </p>
      {#if invitation.words}<p class="words">{invitation.words}</p>{/if}
      <p class="meta">Works once · {clock(left)} left</p>
      <button class="link" onclick={() => (showText = !showText)}
        >{showText ? 'Hide the code as text' : 'Show the code as text'}</button
      >
    </div>
  </div>
  {#if showText}
    <label class="field">
      <span>For a computer without a camera: paste this where it asks for the words.</span>
      <textarea readonly rows="4" value={invitation.qr_text}></textarea>
    </label>
  {/if}
{:else if invitation}
  <p class="hint">This code has run out.</p>
  <div class="actions"><button onclick={make}>Make a new code</button></div>
{:else}
  <p class="waiting"><span class="spinner"></span>Making a code…</p>
{/if}
