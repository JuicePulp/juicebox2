import * as Sentry from "@sentry/node";
import { createRequire } from "node:module";

// Single source of truth for the Sentry release; keep package.json in sync
// (same derivation as astro.config.mjs).
const require = createRequire(import.meta.url);
const pkg = require("./package.json") as { name: string; version: string };

Sentry.init({
  dsn: import.meta.env.SENTRY_DSN_UI || import.meta.env.SENTRY_DSN,
  release: `${pkg.name}@${pkg.version}`,
  environment: import.meta.env.SENTRY_ENVIRONMENT || "production",
  tracesSampleRate: 0.1,
});
