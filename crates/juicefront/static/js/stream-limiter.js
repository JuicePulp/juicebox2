import { PARALLEL_STREAMS } from "./upload-config.js";
let inUse = 0;
export function tryAcquireStream() {
  if (inUse >= PARALLEL_STREAMS)
    return false;
  inUse++;
  return true;
}
export function releaseStream() {
  inUse = Math.max(0, inUse - 1);
}
