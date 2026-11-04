import type { Node as ProseMirrorNode } from '@tiptap/pm/model';

/** The longest preview the Rust core keeps (`PREVIEW_CHARS`). */
export const PREVIEW_CHARS = 160;
const TITLE_CHARS = 120;

/** What the note list shows for a note. */
export interface NoteMeta {
  title: string;
  preview: string;
}

/**
 * A note's title and preview, as in Apple Notes: the first line with text is
 * the title, and the text after it is the preview.
 */
export function noteMeta(doc: ProseMirrorNode): NoteMeta {
  const lines: string[] = [];
  doc.descendants((node) => {
    if (node.isTextblock) {
      const text = node.textContent.replace(/\s+/g, ' ').trim();
      if (text) lines.push(text);
      return false;
    }
    return true;
  });
  const [title = '', ...rest] = lines;
  return {
    title: title.slice(0, TITLE_CHARS),
    preview: rest.join(' ').slice(0, PREVIEW_CHARS),
  };
}
