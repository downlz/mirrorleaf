import { mergeAttributes, Node } from '@tiptap/core';
import { parsePhotoSrc } from './photo-src';

/** A photo as the editor places it. `src` is the note's reference to it. */
export interface PhotoAttrs {
  src: string;
  width: number;
  height: number;
}

export interface PhotoOptions {
  /** The address the screen loads a photo from, for its `src`. */
  url: (src: string) => string;
}

/**
 * TipTap's image node as Mirrorleaf stores it: `image` with `src`, `width` and
 * `height` attributes, the same element the Rust core writes and reads
 * (`add_photo`, `note_blocks`). The note keeps the `mirrorleaf-blob://`
 * reference; only the rendered `<img>` uses the protocol's address.
 */
export const Photo = Node.create<PhotoOptions>({
  name: 'image',
  group: 'block',
  draggable: true,
  atom: true,

  addOptions() {
    return { url: () => '' };
  },

  addAttributes() {
    return {
      src: { default: null },
      width: { default: null },
      height: { default: null },
    };
  },

  parseHTML() {
    return [
      {
        tag: 'img[data-src]',
        getAttrs: (element) => {
          const src = element.getAttribute('data-src') ?? '';
          if (!parsePhotoSrc(src)) return false;
          return {
            src,
            width: Number(element.getAttribute('width')) || null,
            height: Number(element.getAttribute('height')) || null,
          };
        },
      },
    ];
  },

  renderHTML({ HTMLAttributes }) {
    const src = String(HTMLAttributes.src ?? '');
    return [
      'img',
      mergeAttributes(HTMLAttributes, { src: this.options.url(src), 'data-src': src }),
    ];
  },

  addNodeView() {
    return ({ node }) => {
      const figure = document.createElement('figure');
      figure.className = 'photo';
      const img = document.createElement('img');
      img.draggable = false;
      img.alt = '';
      const show = (attrs: Record<string, unknown>) => {
        const src = String(attrs.src ?? '');
        const photo = parsePhotoSrc(src);
        if (img.dataset.src === src) return;
        img.dataset.src = src;
        img.dataset.full = photo?.full ?? '';
        img.src = this.options.url(src);
        const width = Number(attrs.width);
        const height = Number(attrs.height);
        if (width > 0 && height > 0) {
          img.style.aspectRatio = `${width} / ${height}`;
        }
      };
      show(node.attrs);
      figure.append(img);
      return {
        dom: figure,
        update: (updated) => {
          if (updated.type.name !== this.name) return false;
          show(updated.attrs);
          return true;
        },
      };
    };
  },
});
