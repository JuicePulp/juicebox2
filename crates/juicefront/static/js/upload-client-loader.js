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
export const findPausedUpload = client.findPausedUpload;
export const resumeUpload = client.resumeUpload;
