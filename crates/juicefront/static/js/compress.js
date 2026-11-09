import { TEXT_LIKE_RE } from "./upload-config.js?v=2.2.0-r2";
/** Returns `{ file, isGzip }`; falls back to the original on error or under 10% savings. */
export async function compressFile(file) {
  if (file.size < 1024 * 1024)
    return { file, isGzip: false };
  if (!TEXT_LIKE_RE.test(file.name))
    return { file, isGzip: false };
  try {
    const stream = file.stream();
    const cs = new CompressionStream("gzip");
    const [compressed] = stream.pipeThrough(cs);
    const out = await new Response(compressed).arrayBuffer();
    if (out.byteLength < file.size * 0.9) {
      return {
        file: new File([out], file.name + ".gz", {
          type: "application/gzip"
        }),
        isGzip: true
      };
    }
  } catch (err) {
    console.error("Compression failed, using uncompressed file:", err);
  }
  return { file, isGzip: false };
}
