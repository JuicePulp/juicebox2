/** Base64-encode TUS metadata per the TUS protocol spec. */
import { tusSession } from "./api";

export function encodeTusMeta(obj: Record<string, string>): string {
    return Object.entries(obj)
        .map(([k, v]) => `${k} ${btoa(unescape(encodeURIComponent(v)))}`)
        .join(",");
}

/** Best-effort DELETE of a TUS upload by id. */
export function deleteTus(baseUrl: string, tusId: string): void {
  fetch(`${baseUrl}${tusSession(tusId)}`, { method: "DELETE" }).catch(() => {});
}
