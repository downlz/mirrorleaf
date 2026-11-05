import { Extension, type Editor, type Range } from '@tiptap/core';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import Suggestion, { type SuggestionKeyDownProps, type SuggestionProps } from '@tiptap/suggestion';

/** One entry of the slash menu (Notion-style, owner's choice for M5). */
export interface SlashItem {
  id: string;
  title: string;
  hint: string;
  keywords: string[];
  run: (editor: Editor, range: Range) => void;
}

/** The blocks "/" can insert. `pickPhotos` opens the photo picker. */
export function slashItems(pickPhotos: () => void): SlashItem[] {
  const block = (editor: Editor, range: Range) => editor.chain().focus().deleteRange(range);
  return [
    {
      id: 'body',
      title: 'Text',
      hint: 'Plain paragraph',
      keywords: ['paragraph', 'body', 'plain'],
      run: (e, r) => block(e, r).setParagraph().run(),
    },
    {
      id: 'title',
      title: 'Title',
      hint: 'Large heading',
      keywords: ['h1', 'heading'],
      run: (e, r) => block(e, r).setHeading({ level: 1 }).run(),
    },
    {
      id: 'heading',
      title: 'Heading',
      hint: 'Section heading',
      keywords: ['h2'],
      run: (e, r) => block(e, r).setHeading({ level: 2 }).run(),
    },
    {
      id: 'subheading',
      title: 'Subheading',
      hint: 'Small heading',
      keywords: ['h3', 'heading'],
      run: (e, r) => block(e, r).setHeading({ level: 3 }).run(),
    },
    {
      id: 'checklist',
      title: 'Checklist',
      hint: 'Items to tick off',
      keywords: ['todo', 'task', 'check'],
      run: (e, r) => block(e, r).toggleTaskList().run(),
    },
    {
      id: 'bullet',
      title: 'Bulleted list',
      hint: 'Simple list',
      keywords: ['ul', 'unordered', 'list'],
      run: (e, r) => block(e, r).toggleBulletList().run(),
    },
    {
      id: 'numbered',
      title: 'Numbered list',
      hint: 'List with numbers',
      keywords: ['ol', 'ordered', 'list'],
      run: (e, r) => block(e, r).toggleOrderedList().run(),
    },
    {
      id: 'quote',
      title: 'Quote',
      hint: 'Indented quotation',
      keywords: ['blockquote'],
      run: (e, r) => block(e, r).toggleBlockquote().run(),
    },
    {
      id: 'mono',
      title: 'Monostyled',
      hint: 'Code or fixed-width text',
      keywords: ['code', 'pre'],
      run: (e, r) => block(e, r).setCodeBlock().run(),
    },
    {
      id: 'divider',
      title: 'Divider',
      hint: 'Horizontal line',
      keywords: ['hr', 'rule', 'line', 'separator'],
      run: (e, r) => block(e, r).setHorizontalRule().run(),
    },
    {
      id: 'photo',
      title: 'Photo',
      hint: 'From your files',
      keywords: ['image', 'picture', 'heic', 'jpeg'],
      run: (e, r) => {
        block(e, r).run();
        pickPhotos();
      },
    },
  ];
}

/** The items whose title, ID or keywords start with or contain the query. */
export function filterSlashItems(items: SlashItem[], query: string): SlashItem[] {
  const q = query.trim().toLowerCase();
  if (!q) return items;
  const words = (item: SlashItem) => [item.title.toLowerCase(), item.id, ...item.keywords];
  const starts = items.filter((item) => words(item).some((w) => w.startsWith(q)));
  const contains = items.filter(
    (item) => !starts.includes(item) && words(item).some((w) => w.includes(q)),
  );
  return [...starts, ...contains];
}

export interface SlashOptions {
  items: () => SlashItem[];
}

/** Typing "/" opens a menu of blocks to insert. */
export const SlashCommand = Extension.create<SlashOptions>({
  name: 'slashCommand',
  // Before the list extensions, so Enter with the menu open chooses an item
  // rather than splitting a list item.
  priority: 1000,

  addOptions() {
    return { items: () => [] };
  },

  addProseMirrorPlugins() {
    const menu = slashMenu();
    return [
      Suggestion<SlashItem, SlashItem>({
        editor: this.editor,
        char: '/',
        pluginKey: new PluginKey('slashCommand'),
        allowSpaces: false,
        items: ({ query }) => filterSlashItems(this.options.items(), query),
        command: ({ editor, range, props }) => props.run(editor, range),
        render: () => menu,
      }),
      // On Android, ProseMirror ignores the keyboard's Enter key press (it
      // tangles with composition) and reads the new line from the text change
      // instead, so the menu's Enter handling never runs. Catch that change
      // while the menu is open and choose the item instead (M6, Android test 2).
      new Plugin({
        key: new PluginKey('slashEnter'),
        props: {
          // ProseMirror on Chrome Android simulates an Enter when the cursor
          // moves just after the keyboard's Enter, which the chosen item
          // does; that second Enter would undo it (an empty checklist item
          // left again). Swallow it.
          handleKeyDown: (_view, event) => event.key === 'Enter' && menu.justChose(),
          handleDOMEvents: {
            beforeinput: (_view, event) => {
              const type = (event as InputEvent).inputType;
              if (type !== 'insertParagraph' && type !== 'insertLineBreak') return false;
              if (!menu.isOpen()) return false;
              event.preventDefault();
              // Choose a moment later. The keyboard may have just changed the
              // text (Gboard's autocorrect), which ProseMirror reads 20 ms on;
              // changing the note before then loses that read, and ProseMirror's
              // Chrome Android workaround then simulates a Backspace that undoes
              // the chosen block.
              menu.armChoice();
              setTimeout(() => menu.chooseSelected(), CHOOSE_AFTER_MS);
              return true;
            },
          },
        },
      }),
    ];
  },
});

/** How long after Android's Enter the highlighted item is chosen (see `beforeinput`). */
const CHOOSE_AFTER_MS = 40;

/** The menu itself: plain DOM, so the editor package needs no UI framework. */
function slashMenu() {
  let menu: HTMLDivElement | null = null;
  let props: SuggestionProps<SlashItem, SlashItem> | null = null;
  let selected = 0;
  let chosenAt = 0;

  const choose = (index: number) => {
    const item = props?.items[index];
    if (!item || !props) return;
    chosenAt = Date.now();
    props.command(item);
  };

  const draw = () => {
    if (!menu || !props) return;
    menu.replaceChildren();
    if (props.items.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'slash-empty';
      empty.textContent = 'No matches';
      menu.append(empty);
    }
    props.items.forEach((item, index) => {
      const option = document.createElement('button');
      option.type = 'button';
      option.className = 'slash-item';
      option.setAttribute('role', 'option');
      option.setAttribute('aria-selected', String(index === selected));
      const title = document.createElement('span');
      title.className = 'slash-title';
      title.textContent = item.title;
      const hint = document.createElement('span');
      hint.className = 'slash-hint';
      hint.textContent = item.hint;
      option.append(title, hint);
      // Keep the editor's focus and selection: act on press, not on click.
      option.addEventListener('pointerdown', (event) => {
        event.preventDefault();
        choose(index);
      });
      menu?.append(option);
      if (index === selected) option.scrollIntoView({ block: 'nearest' });
    });
    place();
  };

  const place = () => {
    const rect = props?.clientRect?.();
    if (!menu || !rect) return;
    const height = menu.offsetHeight;
    const below = rect.bottom + 6;
    const top = below + height > window.innerHeight ? rect.top - height - 6 : below;
    const left = Math.min(rect.left, window.innerWidth - menu.offsetWidth - 8);
    menu.style.top = `${Math.max(8, top)}px`;
    menu.style.left = `${Math.max(8, left)}px`;
  };

  return {
    /** Whether an item was chosen, or is about to be, in the last moment (see `handleKeyDown`). */
    justChose(): boolean {
      return Date.now() - chosenAt < 200;
    },
    isOpen(): boolean {
      return Boolean(menu && props?.items.length);
    },
    /** Counts a choice as made now, ahead of the delayed choose. */
    armChoice() {
      chosenAt = Date.now();
    },
    /** Chooses the highlighted item, if the menu is open. */
    chooseSelected(): boolean {
      if (!menu || !props?.items[selected]) return false;
      choose(selected);
      return true;
    },
    onStart(start: SuggestionProps<SlashItem, SlashItem>) {
      props = start;
      selected = 0;
      menu = document.createElement('div');
      menu.className = 'slash-menu';
      menu.setAttribute('role', 'listbox');
      document.body.append(menu);
      draw();
    },
    onUpdate(update: SuggestionProps<SlashItem, SlashItem>) {
      props = update;
      selected = 0;
      draw();
    },
    onKeyDown({ event }: SuggestionKeyDownProps) {
      const count = props?.items.length ?? 0;
      if (event.key === 'Escape') {
        menu?.remove();
        menu = null;
        return true;
      }
      if (!menu || count === 0) return false;
      if (event.key === 'ArrowDown') {
        selected = (selected + 1) % count;
        draw();
        return true;
      }
      if (event.key === 'ArrowUp') {
        selected = (selected + count - 1) % count;
        draw();
        return true;
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        choose(selected);
        return true;
      }
      return false;
    },
    onExit() {
      menu?.remove();
      menu = null;
      props = null;
    },
  };
}
