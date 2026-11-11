<script lang="ts">
  // Direction D (owner, 4 Nov 2026): one stream of notes under a search field
  // that is always ready. Typing filters; Enter opens the top match, or
  // creates a note with that title when none has it. Folders are chips.
  import { onMount } from 'svelte';
  import { api, type NoteItem } from '../lib/api';
  import { listDate } from '../lib/dates';
  import { contextMenu } from '../lib/gestures';
  import { displayTitle, highlight, type View } from '../lib/notes';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';

  let { leading = false }: { leading?: boolean } = $props();

  let input: HTMLInputElement | undefined = $state();
  let list: HTMLDivElement | undefined = $state();
  // A folder being named: a new one (id null) or a rename.
  let naming = $state<{ id: string | null; name: string } | null>(null);

  const query = $derived(app.search.trim());
  const folderName = (id: string | null) => app.folders.find((f) => f.id === id)?.name;
  const createIn = $derived(app.view.kind === 'folder' ? folderName(app.view.id) : null);

  const isView = (view: View) =>
    app.view.kind === view.kind &&
    (view.kind !== 'folder' || (app.view.kind === 'folder' && app.view.id === view.id));

  onMount(() => {
    app.focusSearch = (selectAll = true) => {
      input?.focus();
      if (selectAll) input?.select();
    };
    return () => (app.focusSearch = undefined);
  });

  // While typing on a wide screen, the top match is shown beside the stream.
  $effect(() => {
    void query;
    if (app.layout === 'narrow') return;
    const visible = app.visible;
    if (!visible.some((n) => n.id === app.selectedId)) app.selectedId = visible[0]?.id ?? null;
  });

  function show(id: string) {
    app.selectedId = id;
    list?.querySelector<HTMLElement>(`[data-id="${id}"]`)?.scrollIntoView({ block: 'nearest' });
  }

  function keydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    const notes = app.visible;
    const at = notes.findIndex((n) => n.id === app.selectedId);
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const next = notes[event.key === 'ArrowDown' ? at + 1 : Math.max(0, at - 1)];
      if (next) show(next.id);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      const target = notes[at] ?? notes[0];
      if (target && !(app.canCreate && notes.length === 0)) app.openAndFocus(target.id);
      else if (app.canCreate) app.createFromSearch();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      if (app.search) app.search = '';
      else input?.blur();
    }
  }

  function startNaming(id: string | null, name = '') {
    naming = { id, name };
  }

  async function finishNaming() {
    const done = naming;
    naming = null;
    const name = done?.name.trim();
    if (!done || !name) return;
    if (done.id) await app.renameFolder(done.id, name);
    else await app.createFolder(name);
  }

  function focusAndSelect(field: HTMLInputElement) {
    field.focus();
    field.select();
  }

  function folderMenu(id: string, name: string, x: number, y: number) {
    app.openMenu(
      [
        { label: 'Rename Folder', action: () => startNaming(id, name) },
        {
          label: 'Delete Folder…',
          danger: true,
          action: () =>
            app.openMenu(
              [
                {
                  label: `Delete “${name}” and move its notes to Recently Deleted?`,
                  section: true,
                },
                { label: 'Delete Folder', danger: true, action: () => void app.deleteFolder(id) },
                { label: 'Cancel' },
              ],
              x,
              y,
            ),
        },
      ],
      x,
      y,
    );
  }
</script>

{#snippet marked(text: string)}
  {#each highlight(text, query) as part, i (i)}{#if part.match}<mark>{part.text}</mark
      >{:else}{part.text}{/if}{/each}
{/snippet}

{#snippet nameField(placeholder: string)}
  {#if naming}
    <input
      class="chip-input"
      {placeholder}
      aria-label="Folder name"
      bind:value={naming.name}
      use:focusAndSelect
      onblur={finishNaming}
      onkeydown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur();
        if (e.key === 'Escape') naming = null;
      }}
    />
  {/if}
{/snippet}

{#snippet row(note: NoteItem)}
  <button
    class="note-row"
    class:current={note.id === app.selectedId}
    data-id={note.id}
    onclick={() => app.select(note.id)}
    ondblclick={() => app.openAndFocus(note.id)}
    use:contextMenu={(x, y) => {
      app.selectedId = note.id;
      app.openMenu(app.noteActions(note), x, y);
    }}
  >
    <span class="note-head">
      <span class="note-title">{@render marked(displayTitle(note))}</span>
      <span class="note-date">{listDate(note.updated_ms)}</span>
    </span>
    <span class="note-preview">{@render marked(note.preview || 'No additional text')}</span>
    {#if (note.folder && app.view.kind !== 'folder') || (note.pinned && !note.trashed)}
      <span class="note-tags">
        {#if note.folder && app.view.kind !== 'folder'}{folderName(note.folder)}{/if}
        {#if note.folder && app.view.kind !== 'folder' && note.pinned && !note.trashed}
          ·
        {/if}
        {#if note.pinned && !note.trashed}Pinned{/if}
      </span>
    {/if}
  </button>
{/snippet}

<section class="pane stream" class:leading aria-label="Notes">
  <header class="pane-header" data-tauri-drag-region>
    <span class="grow" data-tauri-drag-region></span>
    <button
      class="icon-button"
      class:attention={app.status !== null && !app.status.account}
      title={app.status !== null && !app.status.account ? 'Settings: sync not set up' : 'Settings'}
      onclick={() => app.openSettings()}
    >
      <Icon name="settings" />
    </button>
    <button class="icon-button" title="New note" onclick={() => void app.newNote()}>
      <Icon name="compose" />
    </button>
  </header>

  <div class="search" class:active={query !== ''}>
    <Icon name="search" size={16} />
    <input
      bind:this={input}
      bind:value={app.search}
      id="search"
      type="search"
      placeholder="Search or create"
      aria-label="Search notes, or type a title and press Enter to create one"
      autocomplete="off"
      spellcheck="false"
      onkeydown={keydown}
    />
    {#if app.isDesktop}<kbd>{app.platform === 'macos' ? '⌘K' : 'Ctrl+K'}</kbd>{/if}
  </div>

  <div class="chips" role="group" aria-label="Show">
    <button
      class="chip"
      aria-pressed={isView({ kind: 'all' })}
      onclick={() => app.openView({ kind: 'all' })}
      >All <span class="count">{app.counts.all}</span></button
    >
    <button
      class="chip"
      aria-pressed={isView({ kind: 'pinned' })}
      onclick={() => app.openView({ kind: 'pinned' })}>Pinned</button
    >
    {#each app.folders as folder (folder.id)}
      {#if naming?.id === folder.id}
        {@render nameField('Folder name')}
      {:else}
        <button
          class="chip"
          aria-pressed={isView({ kind: 'folder', id: folder.id })}
          onclick={() => app.openView({ kind: 'folder', id: folder.id })}
          use:contextMenu={(x, y) => folderMenu(folder.id, folder.name, x, y)}>{folder.name}</button
        >
      {/if}
    {/each}
    {#if naming && naming.id === null}
      {@render nameField('New folder')}
    {:else}
      <button class="chip add" title="New folder" onclick={() => startNaming(null)}>+</button>
    {/if}
    {#if app.counts.trash > 0 || app.view.kind === 'trash'}
      <button
        class="chip muted"
        aria-pressed={isView({ kind: 'trash' })}
        onclick={() => app.openView({ kind: 'trash' })}
        >Deleted <span class="count">{app.counts.trash}</span></button
      >
    {/if}
  </div>

  {#if app.fallback}
    <p class="banner">Open Mirrorleaf on {app.fallback} to sync.</p>
  {/if}
  {#if app.backup?.due}
    <!-- The backup reminder (E9): 30 days on, or after a new recovery kit. -->
    <div class="banner with-actions">
      <span class="grow">{app.backup.due}</span>
      <button class="link" onclick={() => app.backUpNow()}>Back up now</button>
      <button
        class="link muted"
        onclick={() => void app.run(api.snoozeBackup).then(() => app.loadStatus())}>Later</button
      >
    </div>
  {/if}

  <div class="pane-body stream-body" bind:this={list}>
    {#if app.canCreate && app.visible.length === 0}
      <button class="create" onclick={() => app.createFromSearch()}>
        <span
          ><kbd>↵</kbd> Create “{query}”{#if createIn}&nbsp;in {createIn}{/if}</span
        >
        <small>no match</small>
      </button>
    {/if}
    {#each app.visible as note (note.id)}{@render row(note)}{/each}
    {#if app.canCreate && app.visible.length > 0}
      <button class="create quiet" onclick={() => app.createFromSearch()}>
        <span
          >Create “{query}”{#if createIn}&nbsp;in {createIn}{/if}</span
        >
      </button>
    {/if}
    {#if app.visible.length === 0 && !app.canCreate}
      <p class="empty">
        {#if app.view.kind === 'trash'}Nothing deleted.{:else if query}No matching notes.{:else}No
          notes yet. Type a title above and press Enter.{/if}
      </p>
    {/if}
  </div>
</section>
