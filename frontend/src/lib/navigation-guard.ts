export const APP_NAVIGATION_REQUEST_EVENT = 'meetily:app-navigation-request';

export interface AppNavigationRequestDetail {
  destination: string;
  proceed: () => void;
}

/**
 * Requests an in-app navigation while allowing an active editor to postpone it.
 * The supplied callback is deliberately retained by the guard so navigation-specific
 * side effects (for example selecting a meeting) happen only after confirmation.
 */
export function requestAppNavigation(
  destination: string,
  proceed: () => void,
): boolean {
  if (typeof window === 'undefined') {
    proceed();
    return true;
  }

  const event = new CustomEvent<AppNavigationRequestDetail>(APP_NAVIGATION_REQUEST_EVENT, {
    cancelable: true,
    detail: { destination, proceed },
  });
  const permitted = window.dispatchEvent(event);
  if (permitted) proceed();
  return permitted;
}
