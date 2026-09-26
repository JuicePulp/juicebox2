export const UPLOAD_URL = "";
export function readMaxFileSize() {
  if (typeof document === "undefined")
    return 524288000;
  const el = document.getElementById("server-config");
  const val = el?.getAttribute("data-max-file-size-bytes");
  const n = val ? Number(val) : NaN;
  return Number.isFinite(n) && n > 0 ? n : 524288000;
}
export function readAllowedTtlHours() {
  if (typeof document === "undefined")
    return [0.5, 1, 6, 12, 24, 72, 168];
  const el = document.getElementById("server-config");
  const val = el?.getAttribute("data-allowed-ttl-hours");
  if (!val)
    return [0.5, 1, 6, 12, 24, 72, 168];
  try {
    const arr = JSON.parse(val);
    if (Array.isArray(arr) && arr.length > 0)
      return arr;
  } catch {}
  return [0.5, 1, 6, 12, 24, 72, 168];
}
export function readDefaultTtlHours() {
  if (typeof document === "undefined")
    return 24;
  const el = document.getElementById("server-config");
  const val = el?.getAttribute("data-default-ttl-hours");
  const n = val ? Number(val) : NaN;
  return Number.isFinite(n) && n > 0 ? n : 24;
}
export function readUploadMode() {
  if (typeof document === "undefined")
    return "standard";
  return document.getElementById("server-config")?.getAttribute("data-upload-mode") || "standard";
}
export const TUS_THRESHOLD = 30 * 1024 * 1024;
export const TUS_START_CHUNK = 16 * 1024 * 1024;
export const TUS_MIN_CHUNK = 8 * 1024 * 1024;
export const TUS_MAX_CHUNK = 64 * 1024 * 1024;
export const TUS_TARGET_CHUNK_SECS = 4;
export const PARALLEL_STREAMS = 10;
export const TUS_MIN_PART = 16 * 1024 * 1024;
export const TUS_MAX_PARTS = 32;
export const PIPELINE_WINDOW = 2;
export const LAST_SPEED_KEY = "juicebox_last_upload_bps";
export const TUNER_INTERVAL_MS = 1200;
export const INITIAL_WORKERS = 2;
export const MAX_PART_RUNS = 2;
export function streamsForRate(bytesPerSec) {
  const mbps = bytesPerSec * 8 / 1e6;
  if (mbps < 10)
    return 8;
  if (mbps < 40)
    return 6;
  if (mbps < 150)
    return 4;
  return 3;
}
export function detectNetTier() {
  try {
    const c = navigator.connection;
    if (!c)
      return "wifi";
    if (c.saveData)
      return "cellular";
    if (c.effectiveType === "slow-2g" || c.effectiveType === "2g" || c.effectiveType === "3g")
      return "cellular";
    if (c.type === "cellular" || c.type === "wimax")
      return "cellular";
    if (c.type === "ethernet")
      return "ethernet";
    if (typeof c.downlink === "number" && c.downlink > 0 && c.downlink < 8)
      return "cellular";
    return "wifi";
  } catch {
    return "wifi";
  }
}
export const TIER_DEFAULT_BPS = {
  cellular: 1500 * 1024,
  wifi: 12 * 1024 * 1024,
  ethernet: 40 * 1024 * 1024
};
export const TIER_CHUNK = {
  cellular: { start: 8 * 1024 * 1024, max: 32 * 1024 * 1024 },
  wifi: { start: 16 * 1024 * 1024, max: 64 * 1024 * 1024 },
  ethernet: { start: 32 * 1024 * 1024, max: 64 * 1024 * 1024 }
};
export const RESERVE_URL = `${UPLOAD_URL}/upload/reserve`;
export const ULTRAFAST_RESERVE_URL = `${UPLOAD_URL}/upload/ultrafast/reserve`;
export const DIRECT_RESERVE_URL = `${UPLOAD_URL}/upload/direct/reserve`;
export const DIRECT_COMPLETE_URL = `${UPLOAD_URL}/upload/direct/complete`;
export const TEXT_LIKE_RE = /\.(txt|json|csv|tsv|xml|html|css|js|ts|jsx|tsx|md|yaml|yml|toml|ini|conf|sh|bash|zsh|fish|py|rb|pl|php|java|c|h|cpp|hpp|rs|go|swift|kt|sql|log|diff|patch|srt|vtt|sub|cfg|env|gitignore|dockerignore|editorconfig|prettierrc|eslintrc|babelrc|Makefile|CMakeLists|Gemfile|Podfile|Dockerfile|svg|wasm|epub|tex|rst|graphql|proto)$/i;
const TEXT_MIME_RE = /^text\/|application\/(json|xml|javascript|x-yaml|yaml|svg\+xml|wasm)/i;
export function gzipEligible(filename, mimeType) {
  return TEXT_LIKE_RE.test(filename) || !!mimeType && TEXT_MIME_RE.test(mimeType);
}
const COMPRESSED_MIME_RE = /^(image|video|audio)\/|application\/(pdf|x-7z-compressed|zip|gzip|x-rar-compressed|x-tar|x-bzip2|x-compressed-tar|xz)$/i;
const COMPRESSED_EXT_RE = /\.(jpe?g|jfif|png|gif|webp|heic|heif|avif|bmp|tiff?|mp4|m4v|mkv|mov|webm|avi|wmv|flv|m4a|mp3|aac|oga|ogg|opus|wav|flac|zip|7z|rar|gz|bz2|xz|tgz|pdf|woff2?|ttf|otf|eot)$/i;
export function isKnownCompressed(filename, mimeType) {
  return !!mimeType && COMPRESSED_MIME_RE.test(mimeType) || !!filename && COMPRESSED_EXT_RE.test(filename);
}
