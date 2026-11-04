// The Mirrorleaf editor: TipTap 3 with Yjs, and its bridge to the Rust engine
// (implementation plan §4, §5.6).

export { CONTENT_FIELD, openNote } from './editor';
export type { NoteBridge, NoteEditor, OpenNoteOptions } from './editor';
export {
  BLOCK_STYLES,
  blockStyle,
  formatState,
  setBlockStyle,
  toggleList,
  toggleMark,
  toggleQuote,
} from './formatting';
export type { BlockStyle, FormatState, ListKind, MarkName } from './formatting';
export { noteMeta, PREVIEW_CHARS } from './meta';
export type { NoteMeta } from './meta';
export type { PhotoAttrs } from './photo';
export { BLOB_SCHEME, blobRef, parsePhotoSrc, photoUrl } from './photo-src';
export type { PhotoHashes } from './photo-src';
export { filterSlashItems, slashItems } from './slash';
export type { SlashItem } from './slash';
export { NOTE_HEADER, tauriBridge } from './tauri-bridge';
