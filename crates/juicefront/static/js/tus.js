import { tusSession } from "./api.js";
/** Every distinct TUS id an item references, including one being added now. */
export function uniqueTusIds(item, extra = []) {
  if (!item) return [];
  const ids = [
    ...extra,
    ...(Array.isArray(item.tusIds) ? item.tusIds : []),
    item.tusId
  ];
  return [...new Set(ids.filter((id) => typeof id === "string" && id))];
}
export function encodeTusMeta(obj) {
  return Object.entries(obj).map(([k, v]) => `${k} ${btoa(unescape(encodeURIComponent(v)))}`).join(",");
}
/** Best-effort DELETE; failures are deliberately ignored. */
export function deleteTus(baseUrl, tusId) {
  fetch(`${baseUrl}${tusSession(tusId)}`, { method: "DELETE" }).catch(() => {});
}
