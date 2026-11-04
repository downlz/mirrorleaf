<script lang="ts">
  import { onMount } from 'svelte';
  import { api, onEngineEvent, onWillClose } from './lib/api';
  import { app } from './lib/state.svelte';
  import Menu from './components/Menu.svelte';
  import NoteView from './components/NoteView.svelte';
  import Settings from './components/Settings.svelte';
  import Stream from './components/Stream.svelte';

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
    return () => {
      clearInterval(timer);
      window.removeEventListener('resize', resize);
      for (const off of unlisten) void off.then((fn) => fn());
    };
  });

  const editable = (target: EventTarget | null) =>
    target instanceof HTMLElement &&
    (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName));

  function keydown(event: KeyboardEvent) {
    if (event.defaultPrevented || app.settings || app.menu) return;
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
        app.settings = true;
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

<div class="app layout-{app.layout}">
  <Stream leading />
  {#if !sheet}
    <NoteView />
  {/if}
</div>

{#if sheet && app.pane === 'note'}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="scrim" onpointerdown={() => app.back()}></div>
  <div class="note-sheet"><NoteView sheet /></div>
{/if}

{#if app.settings}
  <Settings />
{/if}
{#if app.menu}
  <Menu {...app.menu} onclose={() => (app.menu = null)} />
{/if}
{#if app.message}
  <div class="toast" role="status">{app.message}</div>
{/if}
