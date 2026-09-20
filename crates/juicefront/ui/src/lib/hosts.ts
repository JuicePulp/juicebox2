/**
 * Curated storage-node lists for the host selector modal.
 *
 * Official nodes are run by the Juicebox team; unofficial (community) nodes
 * are run by volunteers. Both lists can be extended without a code change:
 *
 *   PUBLIC_OFFICIAL_HOSTS="Box EU|https://box.juicey.dev|EU,..."
 *   PUBLIC_UNOFFICIAL_HOSTS="Alice|https://alice.example.com|US,..."
 *
 * Entries are `Name|https://url|Region`; region is optional. Invalid entries
 * are ignored. Imported by `.astro` frontmatter only (never shipped as-is;
 * the component embeds the resolved lists as data attributes).
 */

export interface HostNode {
  name: string;
  url: string;
  region: string;
  official: boolean;
}

const OFFICIAL_DEFAULTS: HostNode[] = [
  { name: "Box", url: "https://box.juicey.dev", region: "", official: true },
  { name: "F", url: "https://f.juicey.dev", region: "", official: true },
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
    const [name = "", url = "", region = ""] = entry.split("|").map((s) => s.trim());
    const base = normalizeUrl(url);
    if (!base) continue;
    out.push({ name: name || base, url: base, region, official });
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

/** SSR-safe: reads env at render time (works in Astro frontmatter). */
export function getOfficialNodes(): HostNode[] {
  const env =
    typeof process !== "undefined"
      ? process.env.JUICEFRONT_OFFICIAL_HOSTS
      : undefined;
  const clientEnv =
    (import.meta.env.PUBLIC_OFFICIAL_HOSTS as string | undefined) ?? env;
  return mergeNodes(OFFICIAL_DEFAULTS, parseEnvList(clientEnv, true));
}

/** SSR-safe: reads env at render time (works in Astro frontmatter). */
export function getUnofficialNodes(): HostNode[] {
  const env =
    typeof process !== "undefined"
      ? process.env.JUICEFRONT_UNOFFICIAL_HOSTS
      : undefined;
  const clientEnv =
    (import.meta.env.PUBLIC_UNOFFICIAL_HOSTS as string | undefined) ?? env;
  return mergeNodes(UNOFFICIAL_DEFAULTS, parseEnvList(clientEnv, false));
}
