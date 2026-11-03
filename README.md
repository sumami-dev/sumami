# Sumami (数猫咪)

> **Count kitties who visit my websites.** — 由猫开发

Sumami is a **self-hosted web analytics service** that fits on a cheap VPS: one small
binary, one DuckDB file, no external database, no cloud account, no telemetry.

- **Rust + Axum + Tokio** — one self-contained binary, ~50 MB of RAM at rest.
- **DuckDB 2.0.0** embedded — columnar analytics in a single file you can copy around.
- **Umami v2 compatible API** — the existing trackers and dashboards work unchanged.
- **Privacy first** — no cookies, no fingerprinting, **raw IP addresses are never stored**,
  DNT and Global Privacy Control are honoured, and the query string of collected URLs is
  stripped by default.
- **One command to deploy** — `sumami-server serve --db ./sumami.duckdb --port 3000`.

An open-source, high-performance alternative to Umami.

## The repository

| Path       | What it is                                                        |
| ---------- | ----------------------------------------------------------------- |
| `service/` | The server: collection API, stats API, auth, DuckDB writer, TLS.  |
| `cli/`     | `sumami` — read and analyse a database, locally or over HTTP.     |
| `js/`      | The tracker (`sumami.js`) and the dashboard frontend.              |
| `docs/`    | The website and documentation.                                     |

[`AGENTS.md`](AGENTS.md) is the design document: architecture, API contract, privacy
rules, and the conventions every change has to follow.

## Quick start

```bash
git clone https://github.com/sumami-dev/sumami
cd sumami
cargo build --release          # the first build compiles DuckDB: be patient
```

The result is **one** executable in `target/release/`: `sumami`. It is both the server and
the client — `sumami serve …` starts the server, `sumami summary …` reads the data —
because both halves share the schema, the metric definitions and the query code.

DuckDB is compiled into the executable, so the result is a **single file** — no shared
library to install, no `LD_LIBRARY_PATH`, no runtime surprises:

```bash
./target/release/sumami-server serve --db ./sumami.duckdb --port 3000
```

(Prefer a smaller executable over a single file? `make build-small` links the prebuilt
`libduckdb` dynamically instead — then `libduckdb.so` has to sit next to the binary.)

The first run creates the admin account and prints its password **once**. Then:

```bash
# create a website
TOKEN=$(curl -s -X POST http://localhost:3000/api/login \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"<the printed password>"}' | jq -r .token)

curl -s -X POST http://localhost:3000/api/websites \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"name":"My Blog","domain":"example.com"}'
```

Put the tracker on your site:

```html
<script defer src="http://localhost:3000/sumami.js" data-website-id="<website-id>"></script>
```

Open <http://localhost:3000/> for the dashboard, <http://localhost:3000/docs/> for the docs.

## HTTPS without a reverse proxy

```bash
sumami-server serve --db /var/lib/sumami/sumami.duckdb \
                    --ssl example.com --ssl-email you@example.com
```

The server obtains a Let's Encrypt certificate over ACME and renews it on its own.
The challenge is TLS-ALPN-01, so port 443 has to reach Sumami directly; behind a
proxy that terminates TLS, let the proxy manage the certificate instead.

## The CLI

```bash
sumami websites                              # local database
sumami summary <website-id> --from 7d       # or --url https://analytics.example.com
sumami pages <website-id> --from 24h
sumami export <website-id> pages --format csv
```

The same commands work against a local `.duckdb` file and against a running server
(the admin account is used to log in). See `docs/cli.html`.

## Development

```bash
cargo test                                  # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo run -p sumami-cli -- serve --db ./dev.duckdb --port 3000
cargo run -p sumami-cli -- --db ./dev.duckdb summary <website-id>
```

`AGENTS.md` §7 lists the whole workflow.

## What is in the release

```
sumami             one binary: the server (`sumami serve`) and the client (`sumami summary`)
js/                the tracker and the dashboard frontend (published on a CDN)
docs/              the documentation site
```

The binary is self-contained (DuckDB is compiled in) — copy one file, run it.
The dashboard frontend is served from a CDN by default, so the backend never ships it.

## Try it

```bash
sumami serve --db ./demo.duckdb --port 3000   # prints the admin password once
sumami seed --db ./demo.duckdb --days 30       # synthetic traffic (stop the server first)
```

Open <http://localhost:3000/> and sign in.

## Licence

MIT — see [LICENSE](LICENSE).

Third-party software used by Sumami keeps its own licence: DuckDB (MIT),
Axum / Tokio / Hyper (MIT / Apache-2.0), rustls (Apache-2.0 / ISC / MIT).
