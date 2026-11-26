import { type ModListItem, hasPrivateOnlyTag } from '@rsmm/schemas';

const SLUGS_PER_REQUEST = 50;

type RegistryMod = Pick<ModListItem, 'slug' | 'tags'>;
type ListMods = (params: {
  slugs: string[];
  limit: number;
}) => Promise<{ items: RegistryMod[] }>;

/**
 * Resolve current store-side safety labels for installed mods. A registry
 * label can be added after the published archive was built, so local manifest
 * tags alone are not always authoritative for the Library warning.
 */
export async function fetchPrivateOnlyRegistrySlugs(
  slugs: readonly string[],
  listMods: ListMods,
): Promise<Set<string>> {
  const uniqueSlugs = [...new Set(slugs.filter(Boolean))].sort();
  if (uniqueSlugs.length === 0) return new Set();
  const requestedSlugs = new Set(uniqueSlugs);
  const batches: string[][] = [];
  for (let index = 0; index < uniqueSlugs.length; index += SLUGS_PER_REQUEST) {
    batches.push(uniqueSlugs.slice(index, index + SLUGS_PER_REQUEST));
  }

  const pages = await Promise.all(batches.map((batch) => listMods({ slugs: batch, limit: 100 })));
  return new Set(
    pages.flatMap(({ items }) =>
      items
        .filter((mod) => requestedSlugs.has(mod.slug) && hasPrivateOnlyTag(mod.tags))
        .map((mod) => mod.slug),
    ),
  );
}
