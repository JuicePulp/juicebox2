/**
 * Shared error taxonomy for the browser upload pipeline.
 *
 * Every failure carries an INTEGER error code. The legacy upper-snake names are
 * kept as the human-facing mapping (logs, titles, and incoming server strings)
 * but never escape to the UI: the UI always renders `message [1234]`.
 *
 * Number ranges:
 *   1xxx (1000-1019) client/local failures, including UNKNOWN (1019)
 *   2xxx (2000-2004) transport failures (NETWORK_ERROR is never a default)
 *   3xxx (3000-3014) request / session / auth / upload-protocol failures
 *   4xxx (4000-4010) server / storage failures
 *   5xxx (5000-5008) TUS / assembly failures
 *
 * Unknown server codes collapse to UNKNOWN (1019) but are preserved verbatim
 * in the message (`Server code: <RAW>`), so newly deployed codes still surface
 * in the UI instead of silently disappearing.
 */

/** NAME → integer. Frozen: codes are a wire format and must not drift. */
export const E = Object.freeze({
  // 1xxx — client / local
  UPLOAD_FAILED: 1000,
  CLIENT_ABORTED: 1001,
  COMPRESS_FAILED: 1002,
  JSON_PARSE_FAILED: 1003,
  INVALID_REQUEST: 1004,
  FILE_TOO_LARGE: 1005,
  FILE_BLOCKED: 1006,
  QUOTA_EXCEEDED: 1007,
  INVALID_RESPONSE: 1008,
  BAD_RESPONSE: 1009,
  WORKER_LOST: 1010,
  WORKER_MESSAGE_FAILED: 1011,
  UPLOAD_STALLED: 1012,
  IDLE_TIMEOUT: 1013,
  UPLOAD_TIMEOUT: 1014,
  CHECKSUM_MISMATCH: 1015,
  RETRY_EXHAUSTED: 1016,
  EXPIRED: 1017,
  INVALID_ID: 1018,
  UNKNOWN: 1019,

  // 2xxx — transport
  TRANSPORT_ERROR: 2000,
  NETWORK_ERROR: 2001,
  CLOUDFLARE_ERROR: 2002,
  GATEWAY_ERROR: 2003,
  GATEWAY_TIMEOUT: 2004,

  // 3xxx — request / session / auth / upload protocol
  BAD_REQUEST: 3000,
  UNAUTHORIZED: 3001,
  SESSION_EXPIRED: 3002,
  SESSION_INIT_FAILED: 3003,
  SESSION_REJECTED: 3004,
  FORBIDDEN: 3005,
  NOT_FOUND: 3006,
  GONE: 3007,
  METHOD_NOT_ALLOWED: 3008,
  CONFLICT: 3009,
  RATE_LIMITED: 3010,
  RESERVE_FAILED: 3011,
  RESERVE_CONFLICT: 3012,
  TICKET_INVALID: 3013,
  TICKET_EXPIRED: 3014,

  // 4xxx — server / storage
  SERVER_ERROR: 4000,
  INTERNAL_ERROR: 4001,
  SERVER_BUSY: 4002,
  SERVER_UNAVAILABLE: 4003,
  NOT_IMPLEMENTED: 4004,
  LOOP_DETECTED: 4005,
  UNSUPPORTED_MEDIA_TYPE: 4006,
  STORAGE_FULL: 4007,
  UPSTREAM_ERROR: 4008,
  STAGE_FAILED: 4009,
  VERIFY_FAILED: 4010,

  // 5xxx — TUS / assembly
  TUS_ERROR: 5000,
  TUS_CREATE_FAILED: 5001,
  TUS_PATCH_FAILED: 5002,
  TUS_OFFSET_MISMATCH: 5003,
  TUS_COMPLETE_FAILED: 5004,
  CONCAT_FAILED: 5005,
  PART_FAILED: 5006,
  COMPLETE_FAILED: 5007,
  ASSEMBLY_FAILED: 5008,
});

/** integer → legacy name, for logs and human-friendly titles. */
export const ERROR_NAME = Object.freeze(Object.entries(E).reduce((acc, [name, code]) => {
  acc[code] = name;
  return acc;
}, {}));

/** Specific, English-language descriptions keyed by INTEGER code. */
export const ERROR_TEXT = Object.freeze({
  1000: "We don't really know what happened there... Sorry!",
  2000: "The browser could not complete the request to the server",
  2001: "The connection was interrupted before the server replied",
  1014: "The upload took too long and timed out before the server replied",
  1013: "The upload stalled: no data arrived before the server's idle deadline",
  4000: "The server returned an error while handling the upload",
  4002: "The server is busy right now — please try again in a moment",
  4003: "The upload service is temporarily unavailable",
  2003: "An upstream gateway rejected the upload request",
  2004: "An upstream gateway timed out before the upload server replied",
  2002: "The edge network reported an origin failure — retry in a moment",
  3000: "The upload request was malformed or missing required fields",
  3001: "The request was not authorized — refresh the page and try again",
  3002: "The upload session expired — refresh the page and try again",
  3003: "The upload session could not be initialized — refresh the page",
  3004: "The upload session was rejected by the server — refresh and retry",
  3005: "This upload is not permitted by the server's policy",
  3006: "The upload endpoint or file was not found on the server",
  3007: "The upload reservation no longer exists — start a new upload",
  3008: "The server does not allow this request method for uploads",
  3009: "The server reported a conflict with an existing upload",
  3011: "The server did not return a usable upload reservation",
  3012: "Another upload already claimed this reservation — retry",
  3013: "The one-time upload ticket is invalid",
  3014: "The one-time upload ticket expired before the data arrived",
  5001: "The resumable upload session could not be created",
  5002: "A resumable upload chunk could not be delivered",
  5003: "The server's resume offset did not match the client's position",
  5004: "The resumable upload could not be finalized",
  5000: "The resumable upload protocol reported an error",
  5005: "The server could not assemble the uploaded parts into one file",
  1005: "The file is larger than the server allows for this link lifetime",
  1006: "This file type is blocked by the server's safety policy",
  1004: "One or more upload fields failed validation",
  1008: "The server's response could not be understood",
  1003: "The server's JSON response could not be parsed",
  1002: "The file could not be compressed before uploading",
  1015: "The uploaded data did not match its checksum — retry the upload",
  4009: "The server could not move the file into its final storage location",
  4001: "The upload server hit an internal error",
  1001: "The upload was cancelled before it finished",
  1010: "The background upload worker stopped responding",
  1011: "A message to the background upload worker could not be delivered",
  1012: "The upload stopped making progress before it finished",
  3010: "Too many upload requests were made — wait a moment and retry",
  4007: "The server's storage is full — try a shorter retention time",
  1007: "The storage quota for this upload was exceeded",
  4006: "The file's media type is not accepted by the upload endpoint",
  1016: "All automatic retry attempts were used up — try again manually",
  4004: "The server does not implement this upload feature",
  4005: "The server detected a redirect loop while processing the upload",
  1009: "The server returned an unreadable response",
  4008: "An upstream service failed while processing the upload",
  5006: "One or more file parts failed to upload",
  5007: "The server could not complete the upload",
  5008: "The uploaded parts could not be assembled into the final file",
  4010: "The server could not verify the uploaded file",
  1017: "The upload link expired",
  1018: "The upload identifier is invalid",
  1019: "We don't really know what happened there... Sorry!",
});

/** HTTP status → integer code. Status 0 is a failed transport, never a default. */
export const STATUS_CODES = Object.freeze({
  0: E.TRANSPORT_ERROR,
  400: E.BAD_REQUEST,
  401: E.UNAUTHORIZED,
  403: E.FORBIDDEN,
  404: E.NOT_FOUND,
  405: E.METHOD_NOT_ALLOWED,
  408: E.UPLOAD_TIMEOUT,
  409: E.CONFLICT,
  410: E.GONE,
  413: E.FILE_TOO_LARGE,
  415: E.UNSUPPORTED_MEDIA_TYPE,
  422: E.INVALID_REQUEST,
  429: E.RATE_LIMITED,
  500: E.SERVER_ERROR,
  501: E.NOT_IMPLEMENTED,
  502: E.GATEWAY_ERROR,
  503: E.SERVER_BUSY,
  504: E.GATEWAY_TIMEOUT,
  507: E.STORAGE_FULL,
  508: E.LOOP_DETECTED,
  520: E.CLOUDFLARE_ERROR,
  521: E.CLOUDFLARE_ERROR,
  522: E.CLOUDFLARE_ERROR,
  523: E.CLOUDFLARE_ERROR,
  524: E.CLOUDFLARE_ERROR,
});

/** Common backend string codes → integer. All legacy names map to themselves. */
export const SERVER_CODE_MAP = Object.freeze({
  ...Object.fromEntries(Object.keys(E).map((name) => [name, E[name]])),
  TASK_PANICKED: E.INTERNAL_ERROR,
  DATABASE_ERROR: E.INTERNAL_ERROR,
  DB_POOL_ERROR: E.SERVER_BUSY,
  DISK_ERROR: E.SERVER_ERROR,
  INSUFFICIENT_STORAGE: E.STORAGE_FULL,
  INVALID_MULTIPART: E.INVALID_REQUEST,
});

/** Codes that carry no useful detail when they show up inside an Error.message. */
const GENERIC_MESSAGES = new Set([
  "", "network", "network error", "timeout", "aborted", "cancel", "cancelled",
  "upload cancelled", "upload failed", "failed", "error", "unknown error",
  "unexpected error", "http error", "fetch error",
]);

/**
 * Convert any value into an integer code from a documented range.
 * Numbers/digit strings that are known return themselves; unknown ones return
 * UNKNOWN. Legacy names and backend string codes go through SERVER_CODE_MAP.
 * Empty/null means "no explicit code" and returns 0.
 */
export function normalizeCode(value) {
  if (value == null) return 0;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) return E.UNKNOWN;
    if (value === 0) return 0; // neutral "no explicit code"
    return ERROR_NAME[value] ? value : E.UNKNOWN;
  }
  const raw = String(value).trim();
  if (!raw) return 0;
  if (/^\d+$/.test(raw)) {
    const numeric = Number(raw);
    return ERROR_NAME[numeric] ? numeric : E.UNKNOWN;
  }
  const key = raw.toUpperCase().replace(/[\s-]+/g, "_").replace(/[^A-Z0-9_]/g, "");
  if (!key) return E.UNKNOWN;
  const mapped = SERVER_CODE_MAP[key] ?? E[key];
  return Number.isInteger(mapped) && mapped > 0 ? mapped : E.UNKNOWN;
}

function isGenericMessage(message) {
  if (typeof message !== "string") return true;
  const text = message.trim().toLowerCase();
  return GENERIC_MESSAGES.has(text) || /^http\s+\d{3}$/i.test(text);
}

/**
 * Read `{error, message}` / `{code, msg}` JSON bodies and plain-text bodies.
 * `rawCode` is the original string (when present) so unknown codes can be
 * preserved verbatim by classifyFailure.
 */
export function parseServerError(body) {
  const empty = { code: 0, message: "", rawCode: "" };
  if (body == null || body === "") return empty;
  let data = body;
  if (typeof body === "string") {
    const text = body.trim();
    if (!text) return empty;
    try {
      data = JSON.parse(text);
    } catch {
      // Plain-text error body: preserve it verbatim (bounded) instead of losing it.
      return { code: 0, message: text.slice(0, 300), rawCode: "" };
    }
  }
  if (!data || typeof data !== "object" || Array.isArray(data))
    return { code: 0, message: String(body).slice(0, 300), rawCode: "" };

  let rawCode = data.error ?? data.code ?? data.error_code ?? data.status ?? "";
  if (rawCode && typeof rawCode === "object")
    rawCode = rawCode.code ?? rawCode.type ?? "";
  let rawMessage = data.message ?? data.msg ?? data.detail ?? "";
  if (rawMessage && typeof rawMessage === "object")
    rawMessage = rawMessage.message ?? rawCode ?? "";
  const code = normalizeCode(rawCode);
  const message = typeof rawMessage === "string" ? rawMessage.trim().slice(0, 500) : "";
  const preserved = rawCode == null || rawCode === "" ? "" : String(rawCode).slice(0, 120);
  if (!code && !message)
    return { code: 0, message: String(body).slice(0, 300), rawCode: preserved };
  return { code, message, rawCode: preserved };
}

/** Best-effort integer code from an Error.name/Error.message when nothing better is available. */
function codeFromError(err) {
  if (!err) return 0;
  const name = err.name || "";
  const text = typeof err.message === "string" ? err.message : "";
  switch (name) {
    case "AbortError": return E.CLIENT_ABORTED;
    case "TimeoutError": return E.UPLOAD_TIMEOUT;
    case "DataCloneError": return E.WORKER_MESSAGE_FAILED;
    case "SyntaxError": return E.JSON_PARSE_FAILED;
    case "QuotaExceededError": return E.QUOTA_EXCEEDED;
    case "RangeError": return E.INVALID_REQUEST;
    default: break;
  }
  if (name === "TypeError" && /failed to fetch|networkerror|load failed|fetch failed/i.test(text))
    return E.TRANSPORT_ERROR;

  const statusMatch = text.match(/HTTP\s+(\d{3})/i);
  if (statusMatch && STATUS_CODES[Number(statusMatch[1])])
    return STATUS_CODES[Number(statusMatch[1])];

  // Order matters: the most specific patterns must win over generic ones.
  const rules = [
    [/(aborted|interrupted) before completion|chunk dropped|no progress|idle deadline/i, E.IDLE_TIMEOUT],
    [/cancelled|aborted|\babort\b/i, E.CLIENT_ABORTED],
    [/timed?\s*out|timeout/i, E.UPLOAD_TIMEOUT],
    [/tus.{0,24}create|create.{0,24}tus/i, E.TUS_CREATE_FAILED],
    [/tus.{0,24}patch|patch.{0,24}tus/i, E.TUS_PATCH_FAILED],
    [/offset/i, E.TUS_OFFSET_MISMATCH],
    [/tus.{0,24}(complete|final)|(complete|final).{0,24}tus|finali[sz]/i, E.TUS_COMPLETE_FAILED],
    [/concat|merge|assembl/i, E.CONCAT_FAILED],
    [/reservation|reserve/i, E.RESERVE_FAILED],
    [/ticket/i, E.TICKET_INVALID],
    [/session.{0,24}(reject|expired)/i, E.SESSION_REJECTED],
    [/session/i, E.SESSION_INIT_FAILED],
    [/part\s*\d*\s*failed/i, E.PART_FAILED],
    [/checksum|digest|hash mismatch/i, E.CHECKSUM_MISMATCH],
    [/compress|gzip/i, E.COMPRESS_FAILED],
    [/json|parse|unreadable response/i, E.JSON_PARSE_FAILED],
    [/too large|file size|413/i, E.FILE_TOO_LARGE],
    [/blocked|dangerous|disallowed/i, E.FILE_BLOCKED],
    [/rate.?limit|429/i, E.RATE_LIMITED],
    [/storage full|507/i, E.STORAGE_FULL],
    [/quota/i, E.QUOTA_EXCEEDED],
    [/unsupported media|415/i, E.UNSUPPORTED_MEDIA_TYPE],
    [/worker/i, E.WORKER_LOST],
    [/stale|stalled|incomplete/i, E.UPLOAD_STALLED],
    [/retry|exhausted/i, E.RETRY_EXHAUSTED],
    [/internal server error|500/i, E.INTERNAL_ERROR],
    [/forbidden|403/i, E.FORBIDDEN],
    [/unauthoriz|401/i, E.UNAUTHORIZED],
    [/not found|404/i, E.NOT_FOUND],
    [/gone|410/i, E.GONE],
    [/conflict|409/i, E.CONFLICT],
    [/failed to fetch|load failed|fetch failed|networkerror|connection.{0,30}interrupt|connection.{0,30}lost|interrupted/i, E.TRANSPORT_ERROR],
    [/tus/i, E.TUS_ERROR],
  ];
  for (const [pattern, code] of rules) {
    if (pattern.test(text)) return code;
  }
  return 0;
}

/**
 * Turn any upload failure into `{errorCode, errorMessage}` with an INTEGER code.
 *
 * Priority: explicit code/message > server body > HTTP status > err heuristics
 * > UPLOAD_FAILED. NETWORK_ERROR is never a default. When an unknown code is
 * chosen, the raw server string is appended to the message so no information
 * is lost.
 */
export function classifyFailure({ phase, err, status, statusText, body, code, message, item } = {}) {
  let errorCode = 0;
  let errorMessage = "";
  let messageFromServer = false;
  let rawCode = "";

  // 1. Explicit arguments (already parsed from a server response by the caller).
  if (code != null && code !== "") {
    errorCode = normalizeCode(code);
    if (typeof code === "string" && code.trim())
      rawCode = code.trim();
  }
  if (typeof message === "string" && message.trim()) {
    errorMessage = message.trim();
    messageFromServer = true;
  }

  // 2. Server-provided body, JSON or plain text. Always inspect it: even when
  // the caller already parsed a numeric code, the raw string may be needed to
  // preserve an unknown code verbatim.
  if (body != null && body !== "") {
    try {
      const parsed = parseServerError(body);
      if (!errorCode && parsed.code) errorCode = parsed.code;
      if (parsed.rawCode && !rawCode) rawCode = parsed.rawCode;
      if (!errorMessage && parsed.message) {
        errorMessage = parsed.message;
        messageFromServer = true;
      }
    } catch {}
  }

  // 3. HTTP status mapping (including status 0 → TRANSPORT_ERROR).
  if (!errorCode && status != null && status !== "") {
    const mapped = STATUS_CODES[Number(status)];
    if (mapped) errorCode = mapped;
  }

  // 4. Error name / message heuristics.
  if (!errorCode) errorCode = codeFromError(err);

  // 5. Last resort — deliberately never NETWORK_ERROR.
  if (!errorCode) errorCode = E.UPLOAD_FAILED;

  if (!errorMessage) errorMessage = ERROR_TEXT[errorCode] || ERROR_TEXT[E.UPLOAD_FAILED];

  // Surface the HTTP status when the message did not come from the server.
  if (!messageFromServer && status != null && status !== "" && Number(status) !== 0) {
    const label = statusText ? "HTTP " + status + " " + statusText : "HTTP " + status;
    if (!errorMessage.includes(label) && !errorMessage.includes("HTTP " + status))
      errorMessage += (errorMessage ? " (" : "") + label + (errorMessage ? ")" : "");
  }

  // Keep useful original error detail without duplicating the chosen message.
  if (err && typeof err.message === "string" && !isGenericMessage(err.message)) {
    const original = err.message.trim();
    if (original !== String(errorCode) && original !== ERROR_NAME[errorCode] && !errorMessage.includes(original))
      errorMessage += (errorMessage ? " — " : "") + original;
  }

  // An unrecognised server code must survive even though the UI shows [1019].
  if (errorCode === E.UNKNOWN && rawCode && !errorMessage.includes("Server code: " + rawCode))
    errorMessage += (errorMessage ? " — " : "") + "Server code: " + rawCode;

  try {
    if (typeof console !== "undefined" && console && typeof console.error === "function")
      console.error("upload failure", {
        phase,
        item: item?.id,
        code: errorCode,
        name: ERROR_NAME[errorCode] || ERROR_NAME[E.UNKNOWN],
        status,
        err,
        body,
      });
  } catch {}

  return { errorCode, errorMessage };
}

/**
 * Render a failure for the UI as `message [1234]`. Accepts integers, digit
 * strings and legacy names; the displayed code is always the integer.
 */
export function formatError(errorCode, rawMessage) {
  const code = normalizeCode(errorCode);
  let message = typeof rawMessage === "string" ? rawMessage.trim() : "";
  if (!message && code)
    message = ERROR_TEXT[code] || ERROR_TEXT[E.UPLOAD_FAILED];
  if (!message)
    message = ERROR_TEXT[E.UPLOAD_FAILED];
  const displayCode = code || E.UPLOAD_FAILED;
  if (message.includes("[" + displayCode + "]"))
    return message;
  return message + " [" + displayCode + "]";
}
