import type { SupportedUiLocale } from "./types";

export type DateTimeFormatOptions = Intl.DateTimeFormatOptions;
export type NumberFormatOptions = Intl.NumberFormatOptions;

export function formatDateTime(
  value: Date | number,
  locale: SupportedUiLocale,
  options: DateTimeFormatOptions = {
    dateStyle: "medium",
    timeStyle: "short",
  },
): string {
  return new Intl.DateTimeFormat(locale, options).format(value);
}
export function formatNumber(
  value: number,
  locale: SupportedUiLocale,
  options?: NumberFormatOptions,
): string {
  return new Intl.NumberFormat(locale, options).format(value);
}

export function formatPercent(
  value: number,
  locale: SupportedUiLocale,
  options?: Omit<NumberFormatOptions, "style">,
): string {
  return formatNumber(value, locale, { ...options, style: "percent" });
}

export function formatRelativeTime(
  value: number,
  unit: Intl.RelativeTimeFormatUnit,
  locale: SupportedUiLocale,
  options: Intl.RelativeTimeFormatOptions = { numeric: "auto" },
): string {
  return new Intl.RelativeTimeFormat(locale, options).format(value, unit);
}

export function formatLanguageName(
  languageCode: string,
  locale: SupportedUiLocale,
): string {
  return (
    new Intl.DisplayNames([locale], { type: "language" }).of(languageCode) ??
    languageCode
  );
}
