import { describe, expect, it, vi } from 'vitest';
import { fetchPrivateOnlyRegistrySlugs } from './private-only-registry';

describe('fetchPrivateOnlyRegistrySlugs', () => {
  it('merges server-side labels for exact installed slugs, including legacy tags', async () => {
    const listMods = vi.fn(async ({ slugs }: { slugs: string[]; limit: number }) => ({
      items: [
        { slug: slugs[0] ?? '', tags: ['private-only'] },
        { slug: slugs[1] ?? '', tags: ['cheats'] },
        { slug: 'ordinary', tags: ['private-only'] },
      ],
    }));

    const result = await fetchPrivateOnlyRegistrySlugs(['mod-b', 'mod-a', 'mod-a'], listMods);

    expect(listMods).toHaveBeenCalledExactlyOnceWith({ slugs: ['mod-a', 'mod-b'], limit: 100 });
    expect(result).toEqual(new Set(['mod-a', 'mod-b']));
  });

  it('splits large libraries into bounded requests', async () => {
    const listMods = vi.fn(async (_params: { slugs: string[]; limit: number }) => ({ items: [] }));
    const slugs = Array.from({ length: 51 }, (_, index) => `mod-${index}`);

    await fetchPrivateOnlyRegistrySlugs(slugs, listMods);

    expect(listMods).toHaveBeenCalledTimes(2);
    expect(listMods.mock.calls[0]?.[0].slugs).toHaveLength(50);
    expect(listMods.mock.calls[1]?.[0].slugs).toHaveLength(1);
  });

  it('skips the API when there are no installed slugs', async () => {
    const listMods = vi.fn(async () => ({ items: [] }));

    expect(await fetchPrivateOnlyRegistrySlugs([], listMods)).toEqual(new Set());
    expect(listMods).not.toHaveBeenCalled();
  });
});
