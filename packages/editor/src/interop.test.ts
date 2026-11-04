// Plan §9: yrs (Rust) and Yjs (here) must read each other's updates. Both
// sides build the same small note with a fixed client ID, compare it with a
// committed fixture, and read the other side's fixture. Set UPDATE_FIXTURES=1
// to rewrite a fixture after a deliberate change.

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import * as Y from 'yjs';
import { CONTENT_FIELD } from './editor';

const fixture = (name: string) =>
  fileURLToPath(new URL(`../../../crates/core/tests/fixtures/${name}`, import.meta.url));
const FULL = 'a'.repeat(64);
const THUMB = 'b'.repeat(64);

/** The note as TipTap would store it: a formatted paragraph, then a photo. */
function yjsNote(): Uint8Array {
  const doc = new Y.Doc();
  doc.clientID = 1;
  doc.transact(() => {
    const paragraph = new Y.XmlElement('paragraph');
    const text = new Y.XmlText();
    paragraph.insert(0, [text]);
    const image = new Y.XmlElement('image');
    doc.getXmlFragment(CONTENT_FIELD).insert(0, [paragraph, image]);
    text.insert(0, 'Hello ');
    text.insert(6, 'bold', { bold: {} });
    image.setAttribute('src', `mirrorleaf-blob://${FULL}?thumb=${THUMB}`);
    image.setAttribute('width', 2048 as unknown as string);
    image.setAttribute('height', 1536 as unknown as string);
  });
  return Y.encodeStateAsUpdate(doc);
}

describe('Yjs and yrs updates', () => {
  it('Yjs writes the fixture the Rust core reads', () => {
    const path = fixture('yjs-note.bin');
    const update = yjsNote();
    if (process.env.UPDATE_FIXTURES || !existsSync(path)) writeFileSync(path, update);
    expect(new Uint8Array(readFileSync(path))).toEqual(update);
  });

  it('Yjs reads what yrs wrote', () => {
    const doc = new Y.Doc();
    Y.applyUpdate(doc, new Uint8Array(readFileSync(fixture('yrs-note.bin'))));
    const blocks = doc.getXmlFragment(CONTENT_FIELD).toArray() as Y.XmlElement[];
    expect(blocks.map((b) => b.nodeName)).toEqual(['paragraph', 'image']);
    const text = blocks[0]!.toArray()[0] as Y.XmlText;
    expect(text.toDelta()).toEqual([
      { insert: 'From ' },
      { insert: 'Rust', attributes: { italic: {} } },
    ]);
    expect(blocks[1]!.getAttribute('src')).toBe(`mirrorleaf-blob://${THUMB}?thumb=${FULL}`);
    expect(blocks[1]!.getAttribute('width') as unknown).toBe(640);
  });
});
