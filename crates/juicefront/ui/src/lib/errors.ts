import { t, type Locale } from "../i18n";
import type { TranslationKeys } from "../i18n/en";

const VANITY_ERROR_KEYS: Record<string, keyof TranslationKeys> = {
  FILE_TOO_LARGE: "error.file_too_large",
  EXPIRED: "error.expired",
  INVALID_ID: "error.invalid_id",
  NOT_FOUND: "error.not_found",
  RATE_LIMITED: "error.rate_limited",
  UPSTREAM_ERROR: "error.upstream",
  STORAGE_FULL: "error.storage_full",
  NETWORK_ERROR: "error.network",
  UPLOAD_FAILED: "error.upload_failed",
  UPLOAD_TIMEOUT: "error.upload_timeout",
  TUS_ERROR: "error.tus",
  INVALID_REQUEST: "error.invalid_request",
  UNAUTHORIZED: "error.unauthorized",
  UNKNOWN: "error.unknown",
};

/** Resolve a user-facing error message from a code or raw message. */
export function vanityMsg(locale: Locale, errorCode?: string, rawMessage?: string): string {
  if (rawMessage) return rawMessage;
  if (errorCode && VANITY_ERROR_KEYS[errorCode]) return t(locale, VANITY_ERROR_KEYS[errorCode]);
  return t(locale, "error.upload_failed");
}
