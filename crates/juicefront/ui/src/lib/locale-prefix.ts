import type { Locale } from "../i18n";


export function localePrefix(locale: Locale): string {
  return locale === "en" ? "" : `/${locale}`;
}


export function lp(locale: Locale, path: string): string {
  return `${localePrefix(locale)}${path}`;
}
