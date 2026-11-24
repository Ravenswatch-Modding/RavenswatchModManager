import { createContext, useContext } from 'react';

const SettingsExitContext = createContext<(() => void) | null>(null);

export const SettingsExitProvider = SettingsExitContext.Provider;

export function useExitSettings() {
  const exit = useContext(SettingsExitContext);
  if (!exit) throw new Error('Settings navigation requires SettingsExitProvider');
  return exit;
}
