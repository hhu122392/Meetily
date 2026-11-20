let returnPath = '/';

// Keep the entry page while navigating between settings and its subpages.
// A direct launch into settings has no entry page and falls back to home.
export function rememberSettingsReturnPath(
  path = typeof window === 'undefined'
    ? '/'
    : window.location.pathname + window.location.search + window.location.hash,
) {
  if (!path.startsWith('/') || path.startsWith('//') || path.includes('\\')) return;
  const pathname = path.split(/[?#]/)[0];
  if (pathname === '/settings' || pathname.startsWith('/settings/')) return;
  returnPath = path;
}

export function getSettingsReturnPath() {
  return returnPath;
}
