import { queryOptions } from '@tanstack/react-query';
import { useApp } from '../store';
import { listLocalModsForProfile } from './rsmm';

export function localModsQueryOptions(profileId: string) {
  return queryOptions({
    queryKey: ['rsmm', 'list', profileId],
    queryFn: async ({ signal }) => {
      const mods = await listLocalModsForProfile(profileId);
      // Reconcile only a fresh disk read. Replaying cached query data on
      // navigation can mistake a newly installed mod for a deleted folder,
      // permanently removing it from the profile's load order.
      if (mods && !signal.aborted) useApp.getState().syncLocalMods(mods, profileId);
      return mods;
    },
    retry: false,
    // Revisit the folder on entry, including when switching back to a profile
    // whose cached listing is still recent. Install helpers sync the store
    // directly, so the query cache may describe the folder before an install.
    staleTime: 0,
  });
}
