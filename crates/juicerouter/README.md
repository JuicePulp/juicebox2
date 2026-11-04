# juicerouter

Node registry, health probes and routing policy for Juicebox.

`juicerouter` answers two questions:

1. **Where does this upload go?** — `GET /upload-target` returns the best
   writable node for the caller (Cloudflare country → region, then colo,
   then default, then any healthy node).
2. **Where does this file live?** — `GET /v|f|d|e/{path}` resolves the ID
   through juiceback and returns a `302` to the node that stores it (or to
   the R2 URL for hot-tier files), `404` when the file does not exist, and
   `425 Too Early` while an upload is still in progress.

It deliberately **does not** own the byte path for remote files: the
storage node serves the bytes. For files with no `storage_host` (the
home node) it either redirects (`local_mode = "redirect"`) or reverse
proxies (`local_mode = "proxy"`).

## Endpoints

| Endpoint | Purpose |
| --- | --- |
| `GET /health` | `{status, healthy, nodes[]}` — probe state per node |
| `GET /hosts` | Node list with region, URL and health |
| `GET /route/{id}?route=v&path=v/{id}.mp4` | Routing decision as JSON (debug) |
| `GET /upload-target?region=us1` | Best writable node for an upload |
| `GET /v/{path}` etc. | `302` to the owning node / R2, `404`, or `425` |

## Health

Every `probe_interval_secs` the router calls

```
GET {node}/internal/file/00000000/stat
  x-juicehost-api-key: <key>
  x-juiceback-origin: <juiceback_origin>
```

on each node. That route is read-only and returns `200 {"exists":false}`
when the key is accepted, so a healthy node must expose `/internal` and
share the key.

| Status | State | Upload candidate |
| --- | --- | --- |
| `2xx` | `healthy` | yes |
| `401`/`403` | `auth_error` | no |
| `404` | `route_missing` (`/internal` not exposed) | no |
| other / timeout | `unreachable` | no |

`unknown` nodes stay candidates until the first probe finishes, so the
router can start before juiceback/nodes are ready.

## Configuration

Copy `juicerouter.toml.example` to `/var/lib/juicebox/juicerouter.toml`,
point `JUICEROUTER_CONFIG` at it if it lives elsewhere, and set the shared
key in the environment:

```sh
JUICEROUTER_API_KEY=...   # or JUICEHOST_API_KEY
```

`local_mode`:

- `redirect` (default) — the router has its own hostname (e.g.
  `f-r.juicey.dev`) and 302s home-node files to `home_url`.
- `proxy` — the router sits in front of `home_url`; home-node files are
  streamed from `local_upstream` instead of redirected (avoids loops).

## Running

```sh
cargo run -p juicerouter
```

systemd (matching the other units under `/var/lib/juicebox`):

```ini
[Service]
WorkingDirectory=/var/lib/juicebox
EnvironmentFile=/var/lib/juicebox/.env
ExecStart=/var/lib/juicebox/current/bin/juicerouter
```

## Caddy

The router needs Cloudflare's country header, which Caddy forwards by
default. If it is fronted by Cloudflare, keep `CF-IPCountry` in the
request (it is, unless a `header_up -CF-IPCountry` rule strips it).

```caddy
f-r.juicey.dev {
    reverse_proxy 127.0.0.1:6404
}
```
