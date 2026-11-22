import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { listLocalModsForProfile, installModFromIndex, installModVersion } = vi.hoisted(() => ({
  listLocalModsForProfile: vi.fn(),
  installModFromIndex: vi.fn(),
  installModVersion: vi.fn(),
}));

vi.mock('./rsmm', () => ({
  listLocalModsForProfile,
  installModFromIndex,
  installModVersion,
}));

import { type Profile, activeProfile, isEnabledIn, unadoptedMods, useApp } from '../store';
import { localModsQueryOptions } from './local-mods-query';
import { installModIntoProfile } from './profile-mods';
import type { LocalMod } from './rsmm';

const listed = (...ids: string[]): LocalMod[] =>
  ids.map((id) => ({
    id,
    slug: id,
    name: id,
    version: '1.0.0',
    author: null,
    summary: null,
    license: null,
    tags: [],
    enabled: false,
    path: `/mods/${id}`,
    dependencies: {},
    writes: [],
  }));

const profile = (id: string, loadOrder: string[]): Profile => ({
  id,
  name: id,
  loadOrder,
  disabled: new Set(),
  createdAt: '2026-11-21T00:00:00.000Z',
});

let client: QueryClient;

beforeEach(() => {
  vi.resetAllMocks();
  client = new QueryClient({ defaultOptions: { queries: { gcTime: Number.POSITIVE_INFINITY } } });
  useApp.setState({
    profiles: [profile('A', ['existing']), profile('B', ['other'])],
    activeProfileId: 'A',
    installed: [],
    localMods: {},
  });
});

afterEach(() => client.clear());

describe('Library disk reconciliation', () => {
  it.each([undefined, '2.0.0'])(
    'keeps an install enabled when returning to a cached Library (version %s)',
    async (version) => {
      const before = listed('existing');
      const after = listed('existing', 'new-mod');
      listLocalModsForProfile.mockResolvedValue(before);
      await client.fetchQuery(localModsQueryOptions('A'));

      // Browse/detail installs update the store while Library is unmounted.
      installModFromIndex.mockResolvedValue({ ok: true });
      installModVersion.mockResolvedValue({ ok: true });
      listLocalModsForProfile.mockResolvedValueOnce(before).mockResolvedValueOnce(after);
      await installModIntoProfile('new-mod', 'A', { version });

      const finishRead = vi.fn<(mods: LocalMod[]) => void>();
      listLocalModsForProfile.mockReturnValueOnce(
        new Promise<LocalMod[]>((resolve) => finishRead.mockImplementation(resolve)),
      );
      const observer = new QueryObserver(client, localModsQueryOptions('A'));
      const unsubscribe = observer.subscribe(() => {});
      // React Query immediately exposes the old list on mount, even while it
      // fetches. It must never be used as evidence that new-mod was deleted.
      expect(observer.getCurrentResult().data).toEqual(before);
      expect(observer.getCurrentResult().isFetching).toBe(true);
      expect(isEnabledIn(activeProfile(useApp.getState()), 'new-mod')).toBe(true);

      finishRead(after);
      await vi.waitFor(() => expect(observer.getCurrentResult().isFetching).toBe(false));
      const state = useApp.getState();
      expect(state.installed).toEqual(['existing', 'new-mod']);
      expect(isEnabledIn(activeProfile(state), 'new-mod')).toBe(true);
      expect(unadoptedMods(activeProfile(state), state.installed)).toEqual([]);
      unsubscribe();
    },
  );

  it('preserves an installed mod when the Library refresh fails', async () => {
    client.setQueryData(['rsmm', 'list', 'A'], listed('existing'));
    useApp.getState().installMod('new-mod', 'A');
    useApp.getState().syncLocalMods(listed('existing', 'new-mod'), 'A');
    listLocalModsForProfile.mockRejectedValue(new Error('CLI unavailable'));

    await expect(client.fetchQuery(localModsQueryOptions('A'))).rejects.toThrow('CLI unavailable');

    expect(isEnabledIn(activeProfile(useApp.getState()), 'new-mod')).toBe(true);
    expect(useApp.getState().installed).toEqual(['existing', 'new-mod']);
  });

  it('still reconciles a removal confirmed by a fresh disk read', async () => {
    useApp.getState().installMod('removed', 'A');
    useApp.getState().syncLocalMods(listed('existing', 'removed'), 'A');
    listLocalModsForProfile.mockResolvedValue(listed('existing'));

    await client.fetchQuery(localModsQueryOptions('A'));

    expect(activeProfile(useApp.getState()).loadOrder).toEqual(['existing']);
  });

  it('uses the requested profile when a disk read finishes after a profile switch', async () => {
    const finishRead = vi.fn<(mods: LocalMod[]) => void>();
    listLocalModsForProfile.mockReturnValueOnce(
      new Promise<LocalMod[]>((resolve) => finishRead.mockImplementation(resolve)),
    );
    const pending = client.fetchQuery(localModsQueryOptions('A'));
    useApp.getState().setActiveProfile('B');
    useApp.getState().syncLocalMods(listed('other'), 'B');
    finishRead(listed('existing'));
    await pending;

    expect(listLocalModsForProfile).toHaveBeenCalledWith('A');
    expect(useApp.getState().activeProfileId).toBe('B');
    expect(useApp.getState().installed).toEqual(['other']);
    expect(activeProfile(useApp.getState()).loadOrder).toEqual(['other']);
  });

  it('ignores a read cancelled when leaving Library before an install', async () => {
    const finishRead = vi.fn<(mods: LocalMod[]) => void>();
    listLocalModsForProfile.mockReturnValueOnce(
      new Promise<LocalMod[]>((resolve) => finishRead.mockImplementation(resolve)),
    );
    const observer = new QueryObserver(client, localModsQueryOptions('A'));
    const unsubscribe = observer.subscribe(() => {});
    unsubscribe();
    useApp.getState().installMod('new-mod', 'A');
    useApp.getState().syncLocalMods(listed('existing', 'new-mod'), 'A');

    finishRead(listed('existing'));
    // Let the cancelled CLI read finish; it must not write its older snapshot.
    await Promise.resolve();
    expect(isEnabledIn(activeProfile(useApp.getState()), 'new-mod')).toBe(true);
    expect(useApp.getState().installed).toEqual(['existing', 'new-mod']);
  });
});
