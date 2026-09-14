import { invoke } from "@tauri-apps/api/core";
import type { SupportedUiLocale, UiLocalePreference } from "@/i18n/types";

export interface NativeUiLocaleSnapshot {
  preference: UiLocalePreference;
  locale: SupportedUiLocale;
}

export interface NativeErrorPayload {
  code: string;
  params: Record<string, string>;
  debugMessage: string;
}

const NATIVE_ERROR_TRANSLATION_KEYS: Readonly<Record<string, string>> = {
  NATIVE_UNKNOWN: "common:errors.unknown",
  I18N_INVALID_LOCALE: "common:errors.unknown",
  I18N_STATE_UNAVAILABLE: "common:errors.unknown",
  I18N_PERSISTENCE_FAILED: "common:errors.unknown",
  NOTIFICATION_MANAGER_UNAVAILABLE: "common:errors.unknown",
  NOTIFICATION_OPERATION_FAILED: "common:errors.unknown",
  NOTIFICATION_SERIALIZATION_FAILED: "common:errors.unknown",
};

export function isNativeErrorPayload(value: unknown): value is NativeErrorPayload {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<NativeErrorPayload>;
  return (
    typeof candidate.code === "string" &&
    typeof candidate.debugMessage === "string" &&
    Boolean(candidate.params) &&
    typeof candidate.params === "object"
  );
}

export function nativeErrorTranslationKey(value: unknown): string {
  if (!isNativeErrorPayload(value)) return "common:errors.unknown";
  return NATIVE_ERROR_TRANSLATION_KEYS[value.code] ?? "common:errors.unknown";
}

export async function getNativeUiLocale(): Promise<NativeUiLocaleSnapshot | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<NativeUiLocaleSnapshot>("get_ui_locale");
  } catch {
    return null;
  }
}

export async function syncNativeUiLocale(
  preference: UiLocalePreference,
  locale: SupportedUiLocale,
): Promise<NativeUiLocaleSnapshot | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<NativeUiLocaleSnapshot>("set_ui_locale", {
      preference,
      locale,
    });
  } catch {
    // React i18n must remain usable in a browser-only build or when native IPC
    // is temporarily unavailable. Native errors are consumed by callers that
    // explicitly need to present them.
    return null;
  }
}

function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  return Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);
}
