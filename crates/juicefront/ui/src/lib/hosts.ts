













export interface HostNode {
  name: string;
  url: string;
  region: string;
  official: boolean;
  kind: "router" | "storage";
}

const OFFICIAL_DEFAULTS: HostNode[] = [
  { name: "Default", url: "", region: "", official: true, kind: "storage" },
  { name: "Router", url: "https://f.juicey.dev", region: "", official: true, kind: "router" },
  { name: "Europe-1", url: "https://f-eu1.juicey.dev", region: "Europe", official: true, kind: "storage" },
  { name: "Europe-2", url: "https://f-eu2.juicey.dev", region: "Europe", official: true, kind: "storage" },
  // New regions (US, Asia, Oceania, …) get a row here once their DNS exists.
];

const UNOFFICIAL_DEFAULTS: HostNode[] = [];

function normalizeUrl(raw: string): string | null {
  let h = raw.trim();
  if (!h) return null;
  if (!/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(h)) h = `https://${h}`;
  try {
    const url = new URL(h);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    return url.origin;
  } catch {
    return null;
  }
}

function parseEnvList(
  raw: string | undefined,
  official: boolean,
): HostNode[] {
  if (!raw) return [];
  const out: HostNode[] = [];
  for (const entry of raw.split(",")) {
    const [name = "", url = "", region = "", kind = ""] = entry.split("|").map((s) => s.trim());
    const base = normalizeUrl(url);
    if (!base) continue;
    out.push({ name: name || base, url: base, region, official, kind: kind === "router" ? "router" : "storage" });
  }
  return out;
}

function mergeNodes(base: HostNode[], extra: HostNode[]): HostNode[] {
  const seen = new Set(base.map((n) => n.url));
  const out = [...base];
  for (const n of extra) {
    if (seen.has(n.url)) continue;
    seen.add(n.url);
    out.push(n);
  }
  return out;
}


export function getOfficialNodes(): HostNode[] {
  const env =
    typeof process !== "undefined"
      ? process.env.JUICEFRONT_OFFICIAL_HOSTS
      : undefined;
  const clientEnv =
    (import.meta.env.PUBLIC_OFFICIAL_HOSTS as string | undefined) ?? env;
  return mergeNodes(OFFICIAL_DEFAULTS, parseEnvList(clientEnv, true));
}


export function getUnofficialNodes(): HostNode[] {
  const env =
    typeof process !== "undefined"
      ? process.env.JUICEFRONT_UNOFFICIAL_HOSTS
      : undefined;
  const clientEnv =
    (import.meta.env.PUBLIC_UNOFFICIAL_HOSTS as string | undefined) ?? env;
  return mergeNodes(UNOFFICIAL_DEFAULTS, parseEnvList(clientEnv, false));
}
