import { create } from 'zustand';
import type { BrowseSort } from './browse-filter';
import type { LibrarySort, LibraryStatusFilter } from './library-filter';
import type { ModCategory } from './mod-types';

interface LibraryView {
  query: string;
  category: ModCategory | 'all';
  status: LibraryStatusFilter;
  sort: LibrarySort;
}

interface BrowseView {
  tab: 'mods' | 'collections';
  q: string;
  sort: BrowseSort;
  category: string | null;
  tags: string[];
  hideInstalled: boolean;
  minRating: number;
}

/** Keep the listing a user left intact while Settings owns the screen.
 * Session-only: closing or reloading the app starts with the default filters. */
export const useListingViewState = create<{
  library: LibraryView;
  browse: BrowseView;
  updateLibrary: (patch: Partial<LibraryView>) => void;
  updateBrowse: (patch: Partial<BrowseView>) => void;
}>((set) => ({
  library: { query: '', category: 'all', status: 'all', sort: 'load-order' },
  browse: {
    tab: 'mods',
    q: '',
    sort: 'popular',
    category: null,
    tags: [],
    hideInstalled: false,
    minRating: 0,
  },
  updateLibrary: (patch) => set((state) => ({ library: { ...state.library, ...patch } })),
  updateBrowse: (patch) => set((state) => ({ browse: { ...state.browse, ...patch } })),
}));
