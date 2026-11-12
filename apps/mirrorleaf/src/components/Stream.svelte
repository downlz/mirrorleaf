<script lang="ts">
  // Direction D (owner, 4 Nov 2026): one stream of notes under a search field
  // that is always ready. Typing filters; Enter opens the top match, or
  // creates a note with that title when none has it. Folders are chips.
  import { onMount } from 'svelte';
  import { api, type NoteItem } from '../lib/api';
  import { listDate } from '../lib/dates';
  import { contextMenu } from '../lib/gestures';
  import {
    displayTitle,
    folderDescendants,
    folderPath,
    folderTree,
    highlight,
    placeLabel,
    snippetParts,
    type View,
  } from '../lib/notes';
  import { app } from '../lib/state.svelte';
  import Icon from './Icon.svelte';

  let { leading = false }: { leading?: boolean } = $props();

  let input: HTMLInputElement | undefined = $state();
  let list: HTMLDivElement | undefined = $state();
  // A folder being named: a new one (id null) or a rename.
  // A new one can go inside another folder (`parent`, M12).
  let naming = $state<{ id: string | null; name: string; parent?: string | null } | null>(null);

  const query = $derived(app.search.trim());
  const folderName = (id: string | null) => app.folders.find((f) => f.id === id)?.name;
  /** The folder being shown, if any: notes inside it are labelled from there. */
  const viewFolder = $derived(app.view.kind === 'folder' ? app.view.id : null);
  const current = $derived(app.folders.find((f) => f.id === viewFolder) ?? null);
  /** From the top level down to the folder shown, for the breadcrumb. */
  const currentPath = $derived(current ? folderPath(app.folders, current.id) : []);
  /** The chips' folders: inside the folder shown, or at the top level (option A). */
  const levelFolders = $derived(
    app.folders.filter((f) => (current ? f.parent === current.id : f.parent === null)),
  );

  /** Under a note: where it lives below the folder shown ("Clients › Acme"), and "Pinned". */
  const tags = (note: NoteItem): string => {
    const place = note.folder ? placeLabel(app.folders, note.folder, viewFolder) : '';
    const pinned = note.pinned && !note.trashed ? 'Pinned' : '';
    return [place, pinned].filter(Boolean).join(' · ');
  };
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

  // The full-text search runs a moment after typing stops (M12, E5); the
  // title-and-preview filter shows at once.
  $effect(() => {
    const q = query;
    const timer = setTimeout(() => void app.runSearch(q), q ? 150 : 0);
    return () => clearTimeout(timer);
  });

  /** Words around the match in the note's text, when only its text matched. */
  const textMatch = (note: NoteItem): string | null => {
    const snippet = app.hits?.get(note.id);
    if (!snippet || !query) return null;
    const q = query.toLowerCase();
    if (note.title.toLowerCase().includes(q) || note.preview.toLowerCase().includes(q)) return null;
    return snippet;
  };

  function confirmEmptyTrash(event: MouseEvent) {
    const count = app.counts.trash;
    app.openMenuUnder(
      [
        {
          label: `Delete ${count === 1 ? 'this note' : `all ${count} notes`} for good? They leave all your devices and can't be brought back.`,
          section: true,
        },
        { label: 'Empty Recently Deleted', danger: true, action: () => void app.emptyTrash() },
        { label: 'Cancel' },
      ],
      event.currentTarget as HTMLElement,
    );
  }

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

  function startNaming(id: string | null, name = '', parent: string | null = null) {
    naming = { id, name, parent };
  }

  async function finishNaming() {
    const done = naming;
    naming = null;
    const name = done?.name.trim();
    if (!done || !name) return;
    if (done.id) await app.renameFolder(done.id, name);
    else await app.createFolder(name, done.parent ?? null);
  }

  function focusAndSelect(field: HTMLInputElement) {
    field.focus();
    field.select();
  }

  function folderMenu(id: string, name: string, x: number, y: number) {
    app.openMenu(
      [
        { label: 'Rename Folder', action: () => startNaming(id, name) },
        { label: 'New Folder Inside', action: () => startNaming(null, '', id) },
        {
          label: 'Move Folder to…',
          action: () => {
            const inside = new Set([id, ...folderDescendants(app.folders, id)]);
            const current = app.folders.find((f) => f.id === id)?.parent ?? null;
            app.openMenu(
              [
                { label: `Move “${name}” to`, section: true },
                {
                  label: 'Top Level',
                  checked: current === null,
                  action: () => void app.moveFolder(id, null),
                },
                ...folderTree(app.folders)
                  .filter(({ folder }) => !inside.has(folder.id))
                  .map(({ folder, depth }) => ({
                    label: folder.name,
                    depth,
                    checked: current === folder.id,
                    action: () => void app.moveFolder(id, folder.id),
                  })),
              ],
              x,
              y,
            );
          },
        },
        {
          label: 'Delete Folder…',
          danger: true,
          action: () =>
            app.openMenu(
              [
                {
                  label: folderDescendants(app.folders, id).length
                    ? `Delete “${name}” and the folders inside it, and move their notes to Recently Deleted?`
                    : `Delete “${name}” and move its notes to Recently Deleted?`,
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
    {#if textMatch(note)}
      <span class="note-preview"
        >{#each snippetParts(textMatch(note) ?? '') as part, i (i)}{#if part.match}<mark
              >{part.text}</mark
            >{:else}{part.text}{/if}{/each}</span
      >
    {:else}
      <span class="note-preview">{@render marked(note.preview || 'No additional text')}</span>
    {/if}
    {#if tags(note)}
      <span class="note-tags">{tags(note)}</span>
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

  {#if current}
    <!-- Option A (owner, 11 Nov 2026): inside a folder, the way back out,
         then the folder and the folders inside it. -->
    <nav class="crumbs" aria-label="Folders">
      <button class="link" onclick={() => app.leaveTo(null)}>‹ All Notes</button>
      {#each currentPath.slice(0, -1) as folder (folder.id)}
        <span class="crumb-sep" aria-hidden="true">›</span>
        <button class="link" onclick={() => app.leaveTo(folder.id)}>{folder.name}</button>
      {/each}
      <span class="crumb-sep" aria-hidden="true">›</span>
      <b>{current.name}</b>
    </nav>
  {/if}
  <div class="chips" role="group" aria-label="Show">
    {#if current}
      {#if naming?.id === current.id}
        {@render nameField('Folder name')}
      {:else}
        <button
          class="chip"
          aria-pressed="true"
          use:contextMenu={(x, y) => folderMenu(current.id, current.name, x, y)}
          >All of {current.name} <span class="count">{app.counts[current.id] ?? 0}</span></button
        >
      {/if}
    {:else}
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
    {/if}
    {#each levelFolders as folder (folder.id)}
      {#if naming?.id === folder.id}
        {@render nameField('Folder name')}
      {:else}
        <button
          class="chip"
          onclick={() => app.enterFolder(folder.id)}
          use:contextMenu={(x, y) => folderMenu(folder.id, folder.name, x, y)}
          >{folder.name}{#if app.counts[folder.id]}&nbsp;<span class="count"
              >{app.counts[folder.id]}</span
            >{/if}</button
        >
      {/if}
    {/each}
    {#if naming && naming.id === null}
      {@render nameField('New folder')}
    {:else}
      <button
        class="chip add"
        title={current ? `New folder in ${current.name}` : 'New folder'}
        onclick={() => startNaming(null, '', current?.id ?? null)}>+</button
      >
    {/if}
    {#if !current && (app.counts.trash > 0 || app.view.kind === 'trash')}
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
  {#if app.status?.update_needed}
    <!-- E6: a device with no sync protocol in common with this one. -->
    <p class="banner">
      Mirrorleaf on {app.status.update_needed} is a different version and can't sync with this one. Update
      both to the latest version.
    </p>
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
    {#if app.view.kind === 'trash' && app.counts.trash > 0}
      <!-- E5: deleting for good, by hand or after 30 days. -->
      <div class="trash-line">
        <span class="grow">Notes here are deleted for good after 30 days.</span>
        <button class="link" onclick={confirmEmptyTrash}>Empty</button>
      </div>
    {/if}
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
