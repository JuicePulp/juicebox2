
import { tusSession } from "./api";

export function encodeTusMeta(obj: Record<string, string>): string {
    return Object.entries(obj)
        .map(([k, v]) => `${k} ${btoa(unescape(encodeURIComponent(v)))}`)
        .join(",");
}


export function deleteTus(baseUrl: string, tusId: string): void {
  fetch(`${baseUrl}${tusSession(tusId)}`, { method: "DELETE" }).catch(() => {});
}
