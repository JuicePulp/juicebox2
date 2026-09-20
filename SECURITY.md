# Security Policy

Juicebox handles authentication and arbitrary file uploads and server-side fetching, so reports are taken seriously.

## Supported versions

Only the latest commit on the default branch receives security fixes. There are no LTS releases (yet).

## Reporting a vulnerability

**Do not open a public issue for vulnerabilities** please contact me with the security Email: opsec@juicey.dev
It MUST Include the affected component and version/commit, the steps to reproduce, and the impact you believe it has.

## What to include

- What you did, in enough detail to reproduce.
- What you expected to happen vs what happened.
- Logs, requests, or config (pls redact secrets: JWT secrets, API keys, peppers, DSNs).
- Whether the issue needs non-default configuration to trigger.

## Response expectations

- Acknowledgement within 7 days.
- Fix or mitigation timeline depends on severity; critical auth-bypass or remote-code issues take priority over everything else.
- Please allow coordinated disclosure: do not publish details until a fix is available and a reasonable upgrade window has passed.
- We do not offer monetary compensation for security vulnerabilities. (We're kinda poor and trying our best /:)

## Scope notes

- The threat model covers the default configuration. Findings that require `JUICEBACK_ALLOW_PRIVATE_FETCH=1`, disabled auth, or `TRUSTED_PROXY_CIDRS=0.0.0.0/0` are operator misconfiguration, though unclear documentation of those knobs is a valid report.
- Out of scope: spam/abuse of public demo instances, social engineering, and vulnerabilities in third-party dependencies without a Juicebox-specific exploit path (report those upstream).
