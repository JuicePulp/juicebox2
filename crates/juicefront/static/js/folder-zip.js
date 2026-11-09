
const ZIP_LOCAL_SIG = 0x04034b50;
const ZIP_CENTRAL_SIG = 0x02014b50;
const ZIP_EOCD_SIG = 0x06054b50;
const ZIP_VERSION = 20;
const ZIP_FLAG_UTF8 = 0x0800;
const ZIP_METHOD_STORE = 0;
const ZIP_METHOD_DEFLATE = 8;
// No Zip64: sizes, offsets, and entry counts must fit these bounds or the writer throws.
const U32_MAX = 0xffffffff;
const U16_MAX = 0xffff;
const EMPTY = new Uint8Array(0);

function encodeName(name) {
  if (typeof TextEncoder !== "undefined") return new TextEncoder().encode(name);
  return Uint8Array.from(unescape(encodeURIComponent(name)), (c) =>
    c.charCodeAt(0),
  );
}

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[i] = c >>> 0;
  }
  return table;
})();

export function crc32(bytes) {
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) {
    c = CRC_TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

const STORE_EXT_RE =
  /\.(jpe?g|jfif|png|gif|webp|heic|heif|avif|bmp|tiff?|mp4|m4v|mkv|mov|webm|avi|wmv|flv|m4a|mp3|aac|oga|ogg|opus|wav|flac|zip|7z|rar|gz|bz2|xz|zst|tgz|pdf|woff2?|ttf|otf|eot)$/i;

function dosDateTime(ms) {
  const d = new Date(Number.isFinite(ms) && ms > 0 ? ms : Date.now());
  const year = Math.min(2107, Math.max(1980, d.getFullYear()));
  return {
    date: ((year - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate(),
    time:
      (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1),
  };
}

/** Normalises separators and drops `.` and empty segments; `..` becomes `_` so an entry cannot escape the extraction root. */
function normalisePath(path) {
  return String(path || "")
    .replace(/\\/g, "/")
    .split("/")
    .filter((part) => part && part !== ".")
    .map((part) => (part === ".." ? "_" : part))
    .join("/");
}

async function deflateRaw(bytes) {
  if (
    bytes.length < 64 ||
    typeof CompressionStream === "undefined" ||
    typeof Response === "undefined" ||
    typeof Blob.prototype.stream !== "function"
  ) {
    return null;
  }
  try {
    const stream = new Blob([bytes])
      .stream()
      .pipeThrough(new CompressionStream("deflate-raw"));
    const out = new Uint8Array(await new Response(stream).arrayBuffer());
    return out.length < bytes.length ? out : null;
  } catch {
    return null;
  }
}

function getBytes(blob) {
  if (typeof blob.arrayBuffer === "function") return blob.arrayBuffer();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error || new Error("read failed"));
    reader.readAsArrayBuffer(blob);
  });
}

function tooLarge() {
  const err = new Error("folder too large for a non-Zip64 archive");
  err.code = "ZIP_TOO_LARGE";
  return err;
}

/** Assembles a ZIP `Blob`; throws `ZIP_TOO_LARGE` rather than overflow the classic fields. `onProgress` fires per entry. */
export async function createZip(entries, { compress = true, onProgress } = {}) {
  const localParts = [];
  const centralParts = [];
  let offset = 0;

  for (const entry of entries) {
    const isDir = !!entry.dir;
    const name = normalisePath(entry.path) + (isDir ? "/" : "");
    if (!name || name === "/") continue;
    const nameBytes = encodeName(name);
    if (nameBytes.length > U16_MAX) throw tooLarge();

    let method = ZIP_METHOD_STORE;
    let crc = 0;
    let uncompressed = 0;
    let compressed = 0;
    let dataPart = EMPTY;

    if (!isDir) {
      const file = entry.file;
      if (!file) continue;
      const bytes = new Uint8Array(await getBytes(file));
      crc = crc32(bytes);
      uncompressed = bytes.length;
      compressed = uncompressed;
      dataPart = file;
      if (compress && !STORE_EXT_RE.test(name)) {
        const packed = await deflateRaw(bytes);
        if (packed) {
          method = ZIP_METHOD_DEFLATE;
          compressed = packed.length;
          dataPart = packed;
        }
      }
      if (uncompressed > U32_MAX || compressed > U32_MAX) throw tooLarge();
    }

    const { date, time } = dosDateTime(
      entry.file && entry.file.lastModified
        ? entry.file.lastModified
        : Date.now(),
    );

    const local = new Uint8Array(30 + nameBytes.length);
    const lv = new DataView(local.buffer);
    lv.setUint32(0, ZIP_LOCAL_SIG, true);
    lv.setUint16(4, ZIP_VERSION, true);
    lv.setUint16(6, ZIP_FLAG_UTF8, true);
    lv.setUint16(8, method, true);
    lv.setUint16(10, time, true);
    lv.setUint16(12, date, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, compressed, true);
    lv.setUint32(22, uncompressed, true);
    lv.setUint16(26, nameBytes.length, true);
    lv.setUint16(28, 0, true);
    local.set(nameBytes, 30);

    const central = new Uint8Array(46 + nameBytes.length);
    const cv = new DataView(central.buffer);
    cv.setUint32(0, ZIP_CENTRAL_SIG, true);
    cv.setUint16(4, ZIP_VERSION, true);
    cv.setUint16(6, ZIP_VERSION, true);
    cv.setUint16(8, ZIP_FLAG_UTF8, true);
    cv.setUint16(10, method, true);
    cv.setUint16(12, time, true);
    cv.setUint16(14, date, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, compressed, true);
    cv.setUint32(24, uncompressed, true);
    cv.setUint16(28, nameBytes.length, true);
    cv.setUint16(30, 0, true);
    cv.setUint16(32, 0, true);
    cv.setUint16(34, 0, true);
    cv.setUint16(36, 0, true);
    cv.setUint32(38, isDir ? 0x10 : 0, true);
    cv.setUint32(42, offset, true);
    central.set(nameBytes, 46);

    localParts.push(local, dataPart);
    centralParts.push(central);
    offset += local.length + compressed;
    if (offset > U32_MAX) throw tooLarge();
    if (onProgress) onProgress({ name, bytes: uncompressed });
  }

  const count = centralParts.length;
  if (!count) return new Blob([], { type: "application/zip" });
  if (count > U16_MAX) throw tooLarge();

  const centralSize = centralParts.reduce((n, part) => n + part.length, 0);
  const eocd = new Uint8Array(22);
  const ev = new DataView(eocd.buffer);
  ev.setUint32(0, ZIP_EOCD_SIG, true);
  ev.setUint16(4, 0, true);
  ev.setUint16(6, 0, true);
  ev.setUint16(8, count, true);
  ev.setUint16(10, count, true);
  ev.setUint32(12, centralSize, true);
  ev.setUint32(16, offset, true);
  ev.setUint16(20, 0, true);

  return new Blob([...localParts, ...centralParts, eocd], {
    type: "application/zip",
  });
}

/** `readEntries` returns one batch (often 100) at a time, so keep reading until it is empty. */
function readAllEntries(reader) {
  return new Promise((resolve, reject) => {
    const out = [];
    const step = () => {
      reader.readEntries((batch) => {
        if (!batch || !batch.length) {
          resolve(out);
          return;
        }
        out.push(...batch);
        step();
      }, reject);
    };
    step();
  });
}

function entryFile(entry) {
  return new Promise((resolve, reject) => entry.file(resolve, reject));
}

async function walk(entry, prefix, out, onBytes) {
  if (!entry) return;
  if (entry.isFile) {
    const file = await entryFile(entry);
    if (onBytes) onBytes(file);
    out.push({ path: prefix + entry.name, file });
    return;
  }
  if (!entry.isDirectory) return;
  const dirPath = `${prefix}${entry.name}/`;
  const children = await readAllEntries(entry.createReader());
  if (!children.length) {
    // Keep the empty directory so the archive round-trips the dropped tree.
    out.push({ path: dirPath, dir: true });
    return;
  }
  for (const child of children) {
    await walk(child, dirPath, out, onBytes);
  }
}

export async function resolveDroppedFile(item) {
  if (!item) return null;
  if (typeof File !== "undefined" && item instanceof File) return item;
  if (typeof item.file === "function") return entryFile(item);
  return null;
}

/** Must run inside the `drop` handler: `DataTransfer.items` is emptied once the callback returns. */
export function collectDropEntries(dataTransfer) {
  const dirs = [];
  const files = [];
  const items = dataTransfer ? Array.from(dataTransfer.items || []) : [];

  for (const item of items) {
    if (item.kind && item.kind !== "file") continue;
    const entry =
      typeof item.webkitGetAsEntry === "function"
        ? item.webkitGetAsEntry()
        : null;
    if (entry) {
      if (entry.isDirectory) dirs.push(entry);
      else if (entry.isFile) files.push(entry);
      continue;
    }
    const file = typeof item.getAsFile === "function" ? item.getAsFile() : null;
    if (file) files.push(file);
  }

  if (!dirs.length && !files.length && dataTransfer) {
    files.push(...Array.from(dataTransfer.files || []));
  }
  return { dirs, files };
}

export function zipName(name) {
  const base = String(name || "folder").replace(/[\\/]/g, "_") || "folder";
  return /\.zip$/i.test(base) ? base : `${base}.zip`;
}

export async function buildFolderZip(
  dirs,
  { compress = true, onProgress, onBytes } = {},
) {
  const entries = [];
  for (const dir of dirs) {
    await walk(dir, "", entries, onBytes);
  }
  if (!entries.some((entry) => !entry.dir)) return null;
  const blob = await createZip(entries, { compress, onProgress });
  const name = dirs.length === 1 ? zipName(dirs[0].name) : "folders.zip";
  return new File([blob], name, {
    type: "application/zip",
    lastModified: Date.now(),
  });
}
