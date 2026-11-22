import type { Collection } from '@rsmm/schemas';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Loader2, Plus } from 'lucide-react';
import { useRef, useState } from 'react';
import { api } from '../lib/api';
import { useSession } from '../lib/auth-client';
import { useT } from '../lib/i18n-react';
import {
  type InstallProgress,
  installModsIntoProfile,
  isModInstalledInProfile,
  resolveInstallTarget,
} from '../lib/profile-mods';
import { activeProfile, useApp } from '../store';
import { Button } from './chrome';
import { InstalledState } from './installed-state';
import { useToast } from './toast';

export function CollectionInstallButton({
  collection,
  onError,
}: {
  collection: Collection;
  onError: (error: string | null) => void;
}) {
  const t = useT();
  const toast = useToast();
  const queryClient = useQueryClient();
  const { data: session } = useSession();
  const profile = useApp(activeProfile);
  const installed = useApp((s) => s.installed);
  const localMods = useApp((s) => s.localMods);
  const running = useRef(false);
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [failed, setFailed] = useState(false);
  // The index contains only a count. Fetch membership on demand, and share
  // the detail cache so returning to Browse retains the installed indicator.
  const { data, refetch } = useQuery({
    queryKey: ['collections', 'detail', collection.slug, session?.user.id ?? null],
    queryFn: () => api.collections.get(collection.slug),
    enabled: false,
    retry: false,
  });
  const allInstalled =
    !failed &&
    data?.updatedAt === collection.updatedAt &&
    data.mods.length > 0 &&
    data.mods.every((mod) => isModInstalledInProfile(mod.slug, profile, installed, localMods));

  async function install() {
    if (running.current) return;
    running.current = true;
    // Capture this before even fetching the collection's membership.
    const targetProfileId = resolveInstallTarget();
    const profileName = profile.name;
    setProgress({ value: 0, max: collection.modCount, slug: null });
    setFailed(false);
    onError(null);
    try {
      const { data: latest } = await refetch({ throwOnError: true });
      if (!latest?.mods.length) throw new Error(t('This collection has no mods to install.'));
      await installModsIntoProfile(
        latest.mods.map((mod) => mod.slug),
        targetProfileId,
        setProgress,
      );
      await queryClient.invalidateQueries({ queryKey: ['mods', 'list'] });
      toast.push(
        t('Added {name} to “{profile}”.', { name: collection.name, profile: profileName }),
        'success',
      );
    } catch (err) {
      setFailed(true);
      onError(err instanceof Error ? err.message : String(err));
    } finally {
      running.current = false;
      setProgress(null);
    }
  }

  if (allInstalled && !progress) {
    return <InstalledState title={t('Already in "{profile}"', { profile: profile.name })} />;
  }
  const label = progress
    ? t('Installing {done}/{total}…', { done: progress.value, total: progress.max })
    : t('Install {name}', { name: collection.name });
  return (
    <Button
      type="button"
      variant="primary"
      size="sm"
      className="registry-install-button min-w-0 justify-center whitespace-nowrap"
      disabled={progress !== null || collection.modCount === 0}
      aria-label={label}
      aria-busy={progress !== null}
      title={
        collection.modCount === 0
          ? t('This collection has no mods to install.')
          : progress
            ? label
            : t('Download from index + add to "{profile}"', { profile: profile.name })
      }
      onClick={(event) => {
        event.stopPropagation();
        void install();
      }}
    >
      {progress ? (
        <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden="true" />
      ) : (
        <Plus className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
      )}
      <span className="registry-install-label">
        {progress ? `${progress.value}/${progress.max}` : t('install')}
      </span>
    </Button>
  );
}
