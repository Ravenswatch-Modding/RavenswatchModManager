import { oneTimeTokenClient } from 'better-auth/client/plugins';
import { createAuthClient } from 'better-auth/react';
import { getApiUrl } from './api-url';

export const authClient = createAuthClient({
  baseURL: getApiUrl(),
  // Backs the desktop OAuth relay: the deep-link handler exchanges the
  // one-time token minted by the browser flow for this client's session.
  plugins: [oneTimeTokenClient()],
  fetchOptions: {
    credentials: 'include',
  },
  // The API issues a persistent HttpOnly cookie, stored in Tauri's WebView
  // data directory. session.cookieCache is a server option, not client storage.
});

export const { signIn, signUp, signOut, useSession } = authClient;
