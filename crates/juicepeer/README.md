# juicepeer

Peer-to-peer file serving for Juicebox. A native client serves its **public**
files from its own machine; the relay moves requests and file bytes over a
WebSocket tunnel. **No file bytes are ever uploaded to or stored on our
servers.**

## How it works

1. The native client runs a local **juicehost** on `127.0.0.1:6402` and opens an
   outbound WSS connection to `wss://ff.juicey.dev/peer/ws`, authenticated with
   a device JWT.
2. The relay verifies the JWT locally (HS256, using the shared secret) and
   replies with a session ID. Device and user identity come only from the
   verified token, never from the client.
3. The client announces its public file IDs. The relay POSTs them to juiceback
   `POST /internal/peer/authorize` (with the `x-juicehost-api-key` header);
   juiceback accepts only non-expired rows with `tier = "peer"` owned by that
   device and returns accepted/rejected IDs.
4. A browser GET to `ff.juicey.dev/{v,f,d,e}/{id}.{ext}` is mapped by the relay
   to the owning session. The relay forwards the request, with `Range`,
   `If-None-Match`, and `Accept` headers, over the WebSocket to the client. The
   client fetches `http://127.0.0.1:6402/{route}/{id}.{ext}` from its local
   juicehost and streams back a response head plus binary chunks. The relay
   relays status and headers to the browser (with `Cache-Control: no-store`).
5. A ping/pong heartbeat detects liveness. Disconnecting a peer drops its
   session and announced IDs and aborts any in-flight requests.

Control messages are JSON text frames; file bytes are binary frames prefixed
with an 8-byte big-endian request ID. The client targets loopback only, so a
peer can never be made to fetch arbitrary URLs (no SSRF).

## Configuration

Run the relay with `juicepeer-relay`. Non-secret settings come from flags,
environment, or a TOML file (see [`juicepeer.toml.example`](juicepeer.toml.example)).
Secrets are **only** read from the environment:

- `JUICEPEER_JWT_SECRET` (fallback `TICKET_JWT_SECRET`, then `JWT_SECRET`)
- `JUICEPEER_API_KEY` (fallback `JUICEHOST_API_KEY`)

Both secrets are required at startup. Defaults: listen `0.0.0.0:6410`,
`relay_base_url = https://ff.juicey.dev`, `juiceback_url = http://127.0.0.1:6401`.

## Reverse proxy (Caddy)

The relay serves plain HTTP/WebSocket on `127.0.0.1:6410`. Terminate TLS and
proxy `ff.juicey.dev` to it. Caddy 2 forwards WebSocket upgrade headers
automatically, so no special upgrade directives are needed:

```caddyfile
ff.juicey.dev {
    reverse_proxy 127.0.0.1:6410
}
```

## Limitations

- **Public files only.** Peer sharing is for public files; the relay never
  sees private data. IDs are authorized against juiceback, so announcing an ID
  the device does not own is rejected.
- **Peer offline.** If no session owns the requested ID, or the peer is not
  connected, the relay cannot serve the file and returns an error (404/503).
  "Peer offline" is reflected in the web UI. A disconnect also aborts any
  in-flight requests for that peer.
