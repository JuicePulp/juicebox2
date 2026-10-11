import * as Sentry from "@sentry/node";
import { createRequire } from "node:module";



const require = createRequire(import.meta.url);
const pkg = require("./package.json") as { name: string; version: string };

Sentry.init({
  dsn: import.meta.env.SENTRY_DSN_UI || import.meta.env.SENTRY_DSN,
  release: `${pkg.name}@${pkg.version}`,
  environment: import.meta.env.SENTRY_ENVIRONMENT || "production",
  tracesSampleRate: 0.1,
});
