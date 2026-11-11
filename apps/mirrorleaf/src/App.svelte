<script lang="ts">
  import { onMount } from 'svelte';
  import { api, onEngineEvent, onWillClose } from './lib/api';
  import { app } from './lib/state.svelte';
  import Menu from './components/Menu.svelte';
  import KitStep from './components/KitStep.svelte';
  import LockScreen from './components/LockScreen.svelte';
  import NoteView from './components/NoteView.svelte';
  import RemovedPage from './components/RemovedPage.svelte';
  import Settings from './components/Settings.svelte';
  import Stream from './components/Stream.svelte';
  import Welcome from './components/Welcome.svelte';

  // How often the status and the fallback prompt are checked while open. The
  // prompt is time-based (plan §5.9: 90 s after a wake), so it needs a timer.
  const STATUS_EVERY_MS = 5000;

  // Direction D: the stream and the note side by side from 640 px; below
  // that the note opens as a sheet over the stream.
  const sheet = $derived(app.layout === 'narrow');

  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  function refreshSoon() {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => void app.refresh(), 250);
  }

  onMount(() => {
    void app.start().then(() => {
      document.documentElement.dataset.platform = app.platform;
      // Ready to type at once, where there is a keyboard.
      if (window.matchMedia('(pointer: fine)').matches) app.focusSearch?.(false);
    });
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible') void app.loadStatus();
    }, STATUS_EVERY_MS);
    const unlisten = [
      onEngineEvent((event) => {
        if (event.kind === 'notes_changed') refreshSoon();
        else if (event.kind === 'status_changed') void app.loadStatus();
        else if (event.kind === 'open_note') void app.openShared();
        else if (event.kind === 'pairing_changed') void app.loadPairing();
        else if (event.kind === 'lock_changed') void app.checkLock();
        else app.editorHooks.photo?.(event.hash);
      }),
      // The window is closing: send the editor's last edits first.
      onWillClose(async () => {
        await app.editorHooks.close?.();
        await api.closeWindow();
      }),
    ];
    const resize = () => (app.width = window.innerWidth);
    window.addEventListener('resize', resize);
    // Android's back gesture closes the note sheet or Settings.
    const popstate = () => app.onPopState();
    window.addEventListener('popstate', popstate);
    // Going to the background (Android) or hidden: send waiting edits now,
    // and start the app lock's time away (E4). Back in sight: lock if due.
    const hidden = () => {
      if (document.visibilityState === 'hidden') {
        void app.editorHooks.flush?.();
        app.screenLeft();
      } else if (app.locked !== null) {
        void app.checkLock();
      }
    };
    document.addEventListener('visibilitychange', hidden);
    return () => {
      clearInterval(timer);
      window.removeEventListener('resize', resize);
      window.removeEventListener('popstate', popstate);
      document.removeEventListener('visibilitychange', hidden);
      for (const off of unlisten) void off.then((fn) => fn());
    };
  });

  const editable = (target: EventTarget | null) =>
    target instanceof HTMLElement &&
    (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName));

  function keydown(event: KeyboardEvent) {
    if (
      event.defaultPrevented ||
      app.locked !== false ||
      app.settings ||
      app.menu ||
      app.needsAccount ||
      app.status?.kit_pending ||
      app.status?.removed
    )
      return;
    const mod = event.metaKey || event.ctrlKey;
    if (mod && !event.altKey && !event.shiftKey) {
      if (event.key === 'k' || event.key === 'f') {
        event.preventDefault();
        if (sheet) app.back();
        app.focusSearch?.(true);
      } else if (event.key === 'n') {
        event.preventDefault();
        void app.newNote();
      } else if (event.key === ',') {
        event.preventDefault();
        app.openSettings();
      }
      return;
    }
    // Esc in the note goes back to the search field (and closes the sheet).
    if (event.key === 'Escape' && editable(event.target)) {
      if (event.target instanceof HTMLElement && event.target.closest('.ProseMirror')) {
        if (sheet) app.back();
        app.focusSearch?.(false);
      }
      return;
    }
    // Typing outside a text field goes into the search field.
    if (!mod && !event.altKey && event.key.length === 1 && !editable(event.target)) {
      if (sheet && app.pane === 'note') return;
      if (event.key === ' ' && event.target instanceof HTMLButtonElement) return;
      event.preventDefault();
      app.search += event.key;
      app.focusSearch?.(false);
    }
  }
</script>

<svelte:window onkeydown={keydown} />

<!-- The overlays sit inside .app too (they are all fixed), so the layout
     class reaches them: on phones Settings fills the screen and the note's
     sheet gets the narrow margins. -->
<div class="app layout-{app.layout}">
  {#if app.locked !== false}
    <!-- The app lock (E4), or not yet known: nothing of the notes shows. -->
    {#if app.locked}<LockScreen />{/if}
  {:else if app.needsAccount}
    <!-- First run: no account yet (E1, direction A). -->
    <Welcome />
  {:else if app.status?.removed}
    <!-- Another device removed this one (E3). -->
    <RemovedPage />
  {:else}
    <Stream leading />
    {#if !sheet}
      <NoteView />
    {/if}

    {#if sheet && app.pane === 'note'}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="scrim" onpointerdown={() => app.back()}></div>
      <div class="note-sheet"><NoteView sheet /></div>
    {/if}

    {#if app.settings}
      <Settings />
    {/if}

    {#if app.status?.kit_pending}
      <!-- A new recovery kit to save (E2): after replacing the key, or left
           unsaved when the app closed. It can't be skipped. -->
      <div class="scrim"></div>
      <div class="sheet" role="dialog" aria-modal="true" aria-label="Your recovery kit">
        <div class="sheet-body">
          <KitStep ondone={() => void app.loadStatus()} />
        </div>
      </div>
    {/if}
  {/if}
  {#if app.menu && app.locked === false}
    <Menu {...app.menu} onclose={() => (app.menu = null)} />
  {/if}
  {#if app.message}
    <div class="toast" role="status">{app.message}</div>
  {/if}
</div>
