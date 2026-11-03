// Load the upload client through a versioned URL. The entry module is already
// cache-busted, but its unversioned imports are not; without this indirection a
// reload can run stale worker code from the browser's module cache.
const version = (() => {
  try {
    return document.getElementById("server-config")?.getAttribute("data-version") || "";
  } catch {
    return "";
  }
})();

const client = await import(
  version
    ? `./upload-client.js?v=${encodeURIComponent(version)}`
    : "./upload-client.js"
);

export const getUploads = client.getUploads;
export const subscribe = client.subscribe;
export const wasRemoved = client.wasRemoved;
export const enqueueUpload = client.enqueueUpload;
export const connectUploads = client.connectUploads;
export const cancelUpload = client.cancelUpload;
export const removeUpload = client.removeUpload;
export const updateUpload = client.updateUpload;
