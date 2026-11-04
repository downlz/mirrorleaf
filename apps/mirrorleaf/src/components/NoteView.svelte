<script lang="ts">
  import {
    BLOCK_STYLES,
    openNote,
    setBlockStyle,
    tauriBridge,
    toggleList,
    toggleMark,
    toggleQuote,
    type FormatState,
    type ListKind,
    type MarkName,
    type NoteEditor,
  } from '@mirrorleaf/editor';
  import { onDestroy } from 'svelte';
  import { noteDate } from '../lib/dates';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';

  // `sheet`: shown over the stream on a phone-sized screen, with a close button.
  let { leading = false, sheet = false }: { leading?: boolean; sheet?: boolean } = $props();

  let host: HTMLDivElement | undefined = $state();
  let current: NoteEditor | null = null;
  // Editors open and close one at a time, in the order asked.
  let queue: Promise<void> = Promise.resolve();
  let format = $state<FormatState | null>(null);
  let formatOpen = $state(false);

  const note = $derived(app.selected);

  function closeCurrent(): Promise<void> {
    const closing = current;
    current = null;
    format = null;
    return closing ? closing.close().catch((e) => app.fail(e)) : Promise.resolve();
  }

  function open(id: string | null, editable: boolean, element: HTMLDivElement) {
    queue = queue.then(async () => {
      await closeCurrent();
      element.replaceChildren();
      formatOpen = false;
      if (!id) return;
      try {
        current = await openNote({
          id,
          element,
          bridge: tauriBridge,
          editable,
          onMeta: (meta) => app.noteEdited(id, meta),
          onFormat: (state) => (format = state),
          onError: (error) => app.fail(error),
        });
        if (app.fresh === id) {
          app.fresh = null;
          current.editor.commands.focus('end');
        }
      } catch (error) {
        app.fail(error);
      }
    });
  }

  $effect(() => {
    const id = app.selectedId;
    const editable = !app.selectedTrashed;
    if (host) open(id, editable, host);
  });

  app.editorHooks = {
    close: () => (queue = queue.then(closeCurrent)),
    photo: (hash) => current?.refreshPhoto(hash),
    focus: () => current?.editor.commands.focus('end'),
  };

  onDestroy(() => {
    app.editorHooks = {};
    queue = queue.then(closeCurrent);
  });

  // Toolbar buttons act on the editor without taking its focus.
  const keepFocus = (event: Event) => event.preventDefault();
  const mark = (name: MarkName) => current && toggleMark(current.editor, name);
  const list = (kind: ListKind) => current && toggleList(current.editor, kind);

  function focusEnd(event: MouseEvent) {
    if (event.target === event.currentTarget && current && !app.selectedTrashed) {
      current.editor.commands.focus('end');
    }
  }
</script>

<section class="pane note" class:leading class:in-sheet={sheet} aria-label="Note">
  <header class="pane-header" data-tauri-drag-region>
    {#if sheet}
      <button class="icon-button" title="Close" onclick={() => app.back()}>
        <Icon name="down" />
      </button>
    {/if}
    <span class="grow" data-tauri-drag-region></span>
    {#if note && !note.trashed}
      <button
        class="icon-button text"
        class:on={formatOpen}
        title="Format"
        aria-expanded={formatOpen}
        onpointerdown={keepFocus}
        onclick={() => (formatOpen = !formatOpen)}>Aa</button
      >
      <button
        class="icon-button"
        class:on={format?.task}
        title="Checklist"
        onpointerdown={keepFocus}
        onclick={() => list('task')}
      >
        <Icon name="checklist" />
      </button>
      <button class="icon-button" title="Add photos" onclick={() => current?.pickPhotos()}>
        <Icon name="photo" />
      </button>
    {/if}
    {#if note}
      <button
        class="icon-button"
        title="More"
        onclick={(e) => app.openMenuUnder(app.noteActions(note), e.currentTarget)}
      >
        <Icon name="more" />
      </button>
    {/if}
  </header>

  {#if formatOpen && format}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="scrim clear" onpointerdown={() => (formatOpen = false)}></div>
    <div class="format-panel" role="dialog" aria-label="Format">
      <div class="format-styles">
        {#each BLOCK_STYLES as style (style.style)}
          <button
            class="style-{style.style}"
            class:on={format.block === style.style}
            onpointerdown={keepFocus}
            onclick={() => current && setBlockStyle(current.editor, style.style)}
            >{style.label}</button
          >
        {/each}
      </div>
      <div class="format-row">
        <button
          class:on={format.bold}
          title="Bold (⌘B)"
          onpointerdown={keepFocus}
          onclick={() => mark('bold')}><b>B</b></button
        >
        <button
          class:on={format.italic}
          title="Italic (⌘I)"
          onpointerdown={keepFocus}
          onclick={() => mark('italic')}><i>I</i></button
        >
        <button
          class:on={format.underline}
          title="Underline (⌘U)"
          onpointerdown={keepFocus}
          onclick={() => mark('underline')}><u>U</u></button
        >
        <button
          class:on={format.strike}
          title="Strikethrough"
          onpointerdown={keepFocus}
          onclick={() => mark('strike')}><s>S</s></button
        >
      </div>
      <div class="format-row">
        <button
          class:on={format.bullet}
          title="Bulleted list"
          onpointerdown={keepFocus}
          onclick={() => list('bullet')}>• List</button
        >
        <button
          class:on={format.ordered}
          title="Numbered list"
          onpointerdown={keepFocus}
          onclick={() => list('ordered')}>1. List</button
        >
        <button
          class:on={format.quote}
          title="Quote"
          onpointerdown={keepFocus}
          onclick={() => current && toggleQuote(current.editor)}>❝ Quote</button
        >
      </div>
    </div>
  {/if}

  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="pane-body note-scroll" onclick={focusEnd}>
    {#if !note}
      <p class="empty">Pick a note, or type a title in the search field and press Enter.</p>
    {:else}
      {#if note.trashed}
        <p class="banner">
          This note is in Recently Deleted.
          <button class="link" onclick={() => void app.restore(note.id)}>Restore it</button> to edit.
        </p>
      {/if}
      <p class="note-date-line">{noteDate(note.updated_ms)}</p>
    {/if}
    <div class="editor-host" bind:this={host} hidden={!note}></div>
  </div>
</section>
