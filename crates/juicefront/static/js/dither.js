var BAYER_4X4 = [
  [0, 8, 2, 10],
  [12, 4, 14, 6],
  [3, 11, 1, 9],
  [15, 7, 13, 5],
];

export function paintDitherOverlay(zone) {
  if (!zone || zone.querySelector(":scope > canvas.dither-layer")) return null;
  var rect = zone.getBoundingClientRect();
  var w = Math.max(1, Math.round(rect.width));
  var h = Math.max(1, Math.round(rect.height));
  var extra = Math.max(1, Math.round(h * 0.16));
  var canvasH = h + extra;
  var canvas = document.createElement("canvas");
  canvas.className = "dither-layer";
  canvas.width = w;
  canvas.height = canvasH;
  canvas.style.height = canvasH + "px";
  canvas.style.top = "0";
  canvas.style.bottom = "auto";
  var ctx = canvas.getContext("2d");
  if (!ctx) return null;
  var img = ctx.createImageData(w, canvasH);
  var data = img.data;
  var top = Math.floor(h * 0.55);
  for (var y = 0; y < canvasH; y++) {
    var t = y <= top ? 0 : (y - top) / Math.max(1, canvasH - 1 - top);
    var g = Math.min(1, t * 1.15);
    for (var x = 0; x < w; x++) {
      var threshold = BAYER_4X4[y % 4][x % 4] / 16;
      var idx = (y * w + x) * 4;
      if (g > threshold) {
        data[idx] = 0;
        data[idx + 1] = 0;
        data[idx + 2] = 0;
        data[idx + 3] = 255;
      } else {
        data[idx + 3] = 0;
      }
    }
  }
  ctx.putImageData(img, 0, 0);
  zone.prepend(canvas);
  return canvas;
}

export function clearDitherOverlay(zone) {
  if (!zone) return;
  var existing = zone.querySelectorAll(":scope > canvas.dither-layer");
  for (var i = 0; i < existing.length; i++) existing[i].remove();
}

export function repaintDitherOverlay(zone) {
  clearDitherOverlay(zone);
  return paintDitherOverlay(zone);
}
