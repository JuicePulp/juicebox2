import type { Locale } from "../i18n";

/** Locale URL prefix: empty for the default locale, `/<locale>` otherwise. */
export function localePrefix(locale: Locale): string {
  return locale === "en" ? "" : `/${locale}`;
}

/** Prefix a site path with the locale, e.g. `lp(locale, "/files")`. */
export function lp(locale: Locale, path: string): string {
  return `${localePrefix(locale)}${path}`;
}
