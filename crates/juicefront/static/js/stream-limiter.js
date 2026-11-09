import { PARALLEL_STREAMS } from "./upload-config.js?v=2.2.0-r2";
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
