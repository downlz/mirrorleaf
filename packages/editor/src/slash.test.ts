import { describe, expect, it } from 'vitest';
import { filterSlashItems, slashItems } from './slash';

const items = slashItems(() => {});
const ids = (query: string) => filterSlashItems(items, query).map((item) => item.id);

describe('the slash menu', () => {
  it('offers every block with no query', () => {
    expect(ids('')).toHaveLength(items.length);
  });

  it('matches titles and keywords, prefixes first', () => {
    expect(ids('check')).toEqual(['checklist']);
    expect(ids('todo')).toEqual(['checklist']);
    expect(ids('image')).toEqual(['photo']);
    expect(ids('h2')).toEqual(['heading']);
    expect(ids('sub')).toEqual(['subheading']);
    expect(ids('list')).toEqual(['bullet', 'numbered', 'checklist']);
    expect(ids('zzz')).toEqual([]);
  });
});
