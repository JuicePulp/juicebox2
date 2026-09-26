function readBundle() {
  var el = document.getElementById("jb-i18n");
  if (!el) return { locale: "en", dict: {} };
  try {
    var data = JSON.parse(el.textContent || "{}");
    return { locale: data.locale || "en", dict: data.dict || {} };
  } catch {
    return { locale: "en", dict: {} };
  }
}

var bundle = null;
function getBundle() {
  if (!bundle) bundle = readBundle();
  return bundle;
}

export function locale() {
  return getBundle().locale || "en";
}

export function t(locale, key, params) {
  var b = getBundle();
  var value = (b.dict && b.dict[key]) || key;
  if (params) {
    for (var k of Object.keys(params)) {
      value = value.split("{" + k + "}").join(String(params[k]));
    }
  }
  return value;
}

var KNOWN_TTL = {
  0.5: "retention.30m",
  1: "retention.1h",
  6: "retention.6h",
  12: "retention.12h",
  24: "retention.24h",
  72: "retention.3d",
  168: "retention.7d",
};

export function ttlLabel(loc, hours) {
  var known = KNOWN_TTL[hours];
  if (known) return t(loc, known);
  var h = Number.isFinite(hours) ? hours : 24;
  if (h < 1) {
    var mins = Math.round(h * 60);
    return t(loc, mins === 1 ? "retention.minute" : "retention.minutes", { count: mins });
  }
  var days = h / 24;
  if (Number.isInteger(days)) {
    return t(loc, days === 1 ? "retention.day" : "retention.days", { count: days });
  }
  var num = Number.isInteger(h) ? h : Math.round(h * 100) / 100;
  return t(loc, num === 1 ? "retention.hour" : "retention.hours", { count: num });
}
