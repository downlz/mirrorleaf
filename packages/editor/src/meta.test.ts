import { getSchema } from '@tiptap/core';
import { TaskItem, TaskList } from '@tiptap/extension-list';
import { Node } from '@tiptap/pm/model';
import StarterKit from '@tiptap/starter-kit';
import { describe, expect, it } from 'vitest';
import { noteMeta, PREVIEW_CHARS } from './meta';
import { Photo } from './photo';

const schema = getSchema([StarterKit, TaskList, TaskItem, Photo]);
const text = (t: string) => ({ type: 'text', text: t });
const para = (t?: string) => ({ type: 'paragraph', content: t ? [text(t)] : [] });
const doc = (...content: object[]) => Node.fromJSON(schema, { type: 'doc', content });

describe('noteMeta', () => {
  it('takes the first line with text as the title and the rest as the preview', () => {
    const note = doc(
      para(),
      { type: 'heading', attrs: { level: 1 }, content: [text('  Groceries ')] },
      {
        type: 'taskList',
        content: [
          { type: 'taskItem', attrs: { checked: false }, content: [para('milk')] },
          { type: 'taskItem', attrs: { checked: true }, content: [para('eggs')] },
        ],
      },
      { type: 'image', attrs: { src: 'mirrorleaf-blob://x', width: 1, height: 1 } },
      para('and   bread'),
    );
    expect(noteMeta(note)).toEqual({ title: 'Groceries', preview: 'milk eggs and bread' });
  });

  it('caps the preview at the length the Rust core keeps', () => {
    const meta = noteMeta(doc(para('Long'), para('x'.repeat(500))));
    expect(meta.preview).toHaveLength(PREVIEW_CHARS);
  });

  it('gives an empty title to a note without text', () => {
    expect(noteMeta(doc(para()))).toEqual({ title: '', preview: '' });
  });
});
