import { describe, expect, it } from 'vitest';
import { blobRef } from './index';

const HASH = 'af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262';

describe('blobRef', () => {
  it('builds a mirrorleaf-blob reference from a BLAKE3 hash', () => {
    expect(blobRef(HASH)).toBe(`mirrorleaf-blob://${HASH}`);
  });

  it.each([
    ['too short', HASH.slice(1)],
    ['upper-case hex', HASH.toUpperCase()],
    ['not hex', 'z'.repeat(64)],
    ['empty', ''],
  ])('rejects a hash that is %s', (_case, hash) => {
    expect(() => blobRef(hash)).toThrow('not a BLAKE3 hash');
  });
});
