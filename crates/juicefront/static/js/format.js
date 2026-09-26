function formatSize(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  var units = ["B", "KB", "MB", "GB", "TB"];
  var i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  var v = bytes / Math.pow(1024, i);
  return (v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)) + " " + units[i];
}
