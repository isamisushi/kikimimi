# kikimimi static website

The English LP and Starlight manual deploy independently to `kikimimi-site` on
Fly. A Node build produces static HTML/CSS/JS; unprivileged Nginx serves the files.
There is no Rust build, database, or API server in this app.

```sh
npm ci
npm run dev    # Astro preview at /kikimimi/
npm test
npm run build
fly deploy --remote-only --ha=false --no-public-ips
```

Run these commands from `docs/`. `fly.toml` and `Dockerfile` here belong only to
the website. The root Dockerfile/fly.toml still deploy the cloud API/dashboard.
For a complete local routing preview, build this Dockerfile and expose port 8080.

## Production routing

DNS and TLS for `kikimimi.dev` remain attached to `kikimimi-cloud`. Its GET/HEAD
routes `/` and `/kikimimi/*` issue `fly-replay: app=kikimimi-site`. Fly Proxy sends
the original request to the static app in the same organization. The static app
needs no public IP or separate certificate. `KIKIMIMI_SITE_APP` in the root
`fly.toml` selects the destination; without it the cloud's public website routes
return 503, while `/overview` and the APIs remain usable for local development.

- `/`: public LP
- `/kikimimi/overview/`, `/kikimimi/installation/`, etc.: manual
- `/kikimimi/_astro/*`, `/kikimimi/pagefind/*`: static assets and search
- `/kikimimi/`: redirects to `/`
- `/overview`, `/login`, `/join/*`: cloud dashboard
- `/v1/*`, `/web/*`, `/auth/*`, `/activate`: cloud API/auth; never replayed

Static missing files/documents return 404, not an SPA shell. Unrelated paths on
the static app return 404. Cookies, OAuth callback URLs, and CLI endpoints remain
on the existing origin. No client-side redirect to a second domain is used.

## Deployments

The `Deploy static website` workflow deploys only this app on `docs/**` changes
on main. It uses the repository secret `FLY_SITE_DEPLOY_TOKEN`, scoped to
`kikimimi-site`. The initial token expires after one year; rotate it before then
using a new app-scoped Fly deploy token and update that GitHub secret.
GitHub Pages is disabled and is no longer a deployment target.

Cloud routing checks: `KIKIMIMI_SITE_APP=kikimimi-site cargo test -p kikimimi-cloud
--test website_test` from the root, with a built dashboard and local test Postgres.

## macOS downloads

The browser checks the public GitHub Releases API, with pagination and an
8-second timeout per request. A stable `desktop-vX.Y.Z` release must contain both
`kikimimi-desktop-X.Y.Z-aarch64.dmg` and `kikimimi-desktop-X.Y.Z-x86_64.dmg`.
Drafts, prereleases, CLI releases, update archives, and incomplete installer pairs
are excluded. Only publish signed desktop builds under that naming convention;
the LP discovers assets but does not verify code signatures.

Publishing a desktop release updates download availability without rebuilding
the website. Before publication the LP shows preview status and CLI instructions.
API/network/rate-limit errors show a Releases link, also available without JS.
The dashboard illustration is explicitly sample data, not a live workspace.
