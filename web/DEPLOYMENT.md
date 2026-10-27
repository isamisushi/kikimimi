# Local dashboard

This web directory belongs to the OSS CLI/desktop app. It contains local and S3
analytics, local subscription snapshots, storage settings and Cloud connection
instructions. It has no Cloud login, billing, roster or Router management UI.

Build independently:

```sh
npm --prefix web ci
npm --prefix web run build
cargo build --locked -p kikimimi --bin kikimimi
```

Run `kikimimi web` and open the printed token-authenticated URL, or open the
Mac app. Cloud login is optional and happens in the Cloud browser origin via
`kikimimi login`; it is never required to view local history.

For frontend development, start the local server, then run
`KIKIMIMI_API_URL=http://127.0.0.1:<local-port> npm --prefix web run dev`.
First open the local server’s printed token URL, then the dev origin (port 5174)
using the same hostname, 127.0.0.1. The local cookie is shared across those ports. Never point this frontend at the paid Cloud API.

There is no separate hosted OSS dashboard deployment. Public documentation
continues to build from docs/. Cloud owns its hosted dashboard and deployment.
