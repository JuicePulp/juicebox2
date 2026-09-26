import { tusSession } from "./api.js";
export function encodeTusMeta(obj) {
  return Object.entries(obj).map(([k, v]) => `${k} ${btoa(unescape(encodeURIComponent(v)))}`).join(",");
}
export function deleteTus(baseUrl, tusId) {
  fetch(`${baseUrl}${tusSession(tusId)}`, { method: "DELETE" }).catch(() => {});
}
