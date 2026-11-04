import type { Editor } from '@tiptap/core';

/** The paragraph styles the format menu offers, as in Apple Notes. */
export type BlockStyle = 'title' | 'heading' | 'subheading' | 'body' | 'mono';
export type MarkName = 'bold' | 'italic' | 'underline' | 'strike';
export type ListKind = 'bullet' | 'ordered' | 'task';

export const BLOCK_STYLES: { style: BlockStyle; label: string }[] = [
  { style: 'title', label: 'Title' },
  { style: 'heading', label: 'Heading' },
  { style: 'subheading', label: 'Subheading' },
  { style: 'body', label: 'Body' },
  { style: 'mono', label: 'Monostyled' },
];

const HEADING_LEVEL = { title: 1, heading: 2, subheading: 3 } as const;

export function setBlockStyle(editor: Editor, style: BlockStyle): void {
  const chain = editor.chain().focus();
  if (style === 'body') chain.setParagraph().run();
  else if (style === 'mono') chain.setCodeBlock().run();
  else chain.setHeading({ level: HEADING_LEVEL[style] }).run();
}

export function blockStyle(editor: Editor): BlockStyle {
  if (editor.isActive('codeBlock')) return 'mono';
  for (const [style, level] of Object.entries(HEADING_LEVEL)) {
    if (editor.isActive('heading', { level })) return style as BlockStyle;
  }
  return 'body';
}

export function toggleMark(editor: Editor, mark: MarkName): void {
  editor.chain().focus().toggleMark(mark).run();
}

export function toggleList(editor: Editor, kind: ListKind): void {
  const chain = editor.chain().focus();
  if (kind === 'bullet') chain.toggleBulletList().run();
  else if (kind === 'ordered') chain.toggleOrderedList().run();
  else chain.toggleTaskList().run();
}

export function toggleQuote(editor: Editor): void {
  editor.chain().focus().toggleBlockquote().run();
}

/** What the toolbar shows as switched on where the cursor is. */
export interface FormatState {
  block: BlockStyle;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  strike: boolean;
  bullet: boolean;
  ordered: boolean;
  task: boolean;
  quote: boolean;
}

export function formatState(editor: Editor): FormatState {
  return {
    block: blockStyle(editor),
    bold: editor.isActive('bold'),
    italic: editor.isActive('italic'),
    underline: editor.isActive('underline'),
    strike: editor.isActive('strike'),
    bullet: editor.isActive('bulletList'),
    ordered: editor.isActive('orderedList'),
    task: editor.isActive('taskList'),
    quote: editor.isActive('blockquote'),
  };
}
