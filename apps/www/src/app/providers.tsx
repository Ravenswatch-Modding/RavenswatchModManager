'use client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { NavigationGuardProvider } from 'nextjs-nav-guard';
import { useEffect, useState } from 'react';
import { registerImageCache } from '../lib/image-cache';

export function Providers({ children }: { children: React.ReactNode }) {
  useEffect(registerImageCache, []);
  const [qc] = useState(
    () => new QueryClient({ defaultOptions: { queries: { staleTime: 30_000 } } }),
  );
  return (
    <NavigationGuardProvider>
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    </NavigationGuardProvider>
  );
}
