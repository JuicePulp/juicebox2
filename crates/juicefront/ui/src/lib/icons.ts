








const iconModules = import.meta.glob<string>(
  "../icons/*.svg",
  { eager: true, query: "?raw", import: "default" },
);

const ICONS: Record<string, string> = {};
for (const [path, raw] of Object.entries(iconModules)) {
  const name = path.split("/").pop()?.replace(".svg", "") ?? "";
  ICONS[name] = raw;
}


const NAME_MAP: Record<string, string> = {
  "x": "close",
  "report": "flag",
  "back": "corner-up-left",
  "book": "book-open",
  "circle-help": "book-open",
  "message-square": "message",
  "more": "more-horizontal",
  "file-image": "image",
  "file-video": "video",
  "file-text": "file-text",
  "file-archive": "archive",
  "file-code": "code",
  "file-audio": "audio-waveform",
  "file-spreadsheet": "presentation",
  "trash-simple": "trash",
  "brand-bluesky": "bluesky",
  "brand-twitter": "twitter",
  "brand-reddit": "reddit",
  "brand-mastodon": "mastodon",
  "brand-linkedin": "linkedin",
  "brand-telegram": "telegram",
  "brand-whatsapp": "whatsapp",
  "tick": "checkmark",
  "tick-on": "checkmark-on",
  "box": "checkbox",
  "box-on": "checkbox-on",
};






export function getIconSvg(name: string): string {
  const key = NAME_MAP[name] ?? name;
  return ICONS[key] ?? "";
}








let iconCounter = 0;
export function iconSvgHtml(name: string, size = 24, className = "", shapeRendering = "crispEdges"): string {
  const raw = getIconSvg(name);
  if (!raw) return "";
  const uid = `i${iconCounter++}`;
  const cls = className ? ` class="${className}"` : "";
  return raw
    .replace(
      /<svg/,
      `<svg width="${size}" height="${size}"${cls} style="display:block;shape-rendering:${shapeRendering}"`,
    )
    .replace(/id="([^"]+)"/g, `id="$1-${uid}"`)
    .replace(/url\(#([^)]+)\)/g, `url(#$1-${uid})`);
}
