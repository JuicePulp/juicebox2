# juicefront UI

Astro + SolidJS frontend for Juicebox. Served by the `juicefront` Rust
runner, which proxies `/api`, `/upload`, `/file/` to juiceback and `/f/`
to juicehost (see `src/middleware.ts` and `astro.config.mjs`).

## Scripts

- `bun run dev` — local dev server (proxies to `:6401`/`:6402`).
- `bun run build` — standalone Node build (`server.mjs`, used in prod).
- `bun run preview` — preview the production build.
- `bun run gen-docs` — regenerate the vendored Rust API docs.

## Layout

- `src/pages/` — file-routed pages (`admin/` for the dashboard).
- `src/components/` — `.astro` for static UI, `.tsx` Solid islands for
  interactive parts (upload tray, file cards).
- `src/lib/` — client helpers; backend route paths live in `lib/api.ts`,
  tunables in `lib/upload-config.ts`.
- `src/i18n/` — `en` is the source of truth; `fr`, `es`, `ru` must carry
  the same key set.
- `src/styles/` — BEM classes, one file per component, imported globally
  from `layouts/BaseLayout.astro`.

## Conventions

- Relative imports with explicit `.astro` extensions; extensionless `.tsx`.
- User strings via `t(locale, "namespace.key")` — never hardcode English.
- API calls go through `lib/api.ts` path builders, never raw literals.
- Silent `catch {}` for best-effort UI work; `console.error` only for
  diagnostics worth surfacing (health, ban, compression).
