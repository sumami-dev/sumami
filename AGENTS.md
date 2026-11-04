# AGENTS.md — Sumami (数猫咪)

> This file is the **single source of truth for the design** of Sumami.
> Read it before writing code. If code and this document disagree, this document wins
> until it is updated in the same change.

---

## 0. TL;DR

Sumami is a **self-hosted, single-binary web analytics service**: Rust + Axum + Tokio in the
backend, **DuckDB 2.0.0** as the embedded analytics store, and an **Umami v2–compatible HTTP
API** so existing Umami dashboards / trackers keep working.

The deployment target is a **cheap VPS** (1 vCPU / 512 MB RAM), not a cloud platform.
No Cloudflare, no Kubernetes, no external database, no object storage required.
One binary + one DuckDB file = the whole system.

Measured footprint (release binary, DuckDB 2.0.0 statically linked):

| | idle | after 2 000 events + 200 queries |
| --- | ---: | ---: |
| process RSS | 42 MB | 113 MB (allocator retention, see below) |
| DuckDB buffer manager | 0.5 MB | 1.8 MB |
| DuckDB memory limit (declared) | 122 MiB (`SET memory_limit='128MB'`) | — |
| DuckDB threads | = core count (1 on a 1-vCPU box) | — |
| database file | 3.5 MB for 4 000 sessions | — |

The declared limit is a **ceiling, not an allocation**: DuckDB's buffer manager held
0.5–1.8 MB while serving traffic. The RSS growth after a burst is the C allocator
keeping freed pages, not live data. `SUMAMI_DUCKDB_MEMORY_LIMIT` tunes the ceiling;
`/api/heartbeat` reports the usage, the configured limit, the limit DuckDB itself
reports back, and the thread count.

### 4.1a Designed for a 512 MB VPS

The defaults are chosen for the deployment target, not for a workstation:

| Setting | Default | Why |
| --- | --- | --- |
| `SUMAMI_DUCKDB_MEMORY_LIMIT` | **min(128 MB, RAM / 4)** | DuckDB's own default is 80% of RAM — 410 MB on a 512 MB box, which would leave nothing for the OS. The limit caps the *buffer*, not the data: queries stream and spill to disk. |
| `SUMAMI_DUCKDB_FLUSH_THRESHOLD` | 32 MB | DuckDB's default (128 MB) keeps that much freed memory before returning it to the OS. |
| `SUMAMI_DUCKDB_TEMP_LIMIT` | 2 GB | A query that spills must not fill the disk. |
| `SUMAMI_DUCKDB_THREADS` | auto (one per 64 MB of budget) | A parallel `COUNT(DISTINCT …)` builds **one hash table per thread**, so a query's memory scales with the thread count: at 10 threads a 5M-row summary query runs out of a 128 MB limit, at 2 threads it takes 154 ms. |
| `SUMAMI_STORE_TITLES` | off | The page title is roughly half of the stored bytes and almost nobody looks at it. |
| `SUMAMI_STORE_FULL_REFERRER` | off | The referrer breakdown groups by host (that is what Umami shows too), and the full URL is the largest field we would store. |

Measured on 10M and 100M row tables (`cli/examples/scale.rs`, event-row schema,
128 MB DuckDB limit, the production thread derivation — 2 threads on a 128 MB budget):

| | 10M rows | 20M rows, realistic cardinality |
| --- | ---: | ---: |
| summary (30-day window) | 98 ms | 434 ms |
| timeseries | 12 ms | 40 ms |
| pages (top 50) | 5 ms | 158 ms |
| referrers | 5 ms | 18 ms |
| browsers | 25 ms | 105 ms |
| **sessions (50 rows)** | **675 ms** | **2 957 ms** |

All of it inside a **128 MB** DuckDB limit. The limit bounds RAM, not history:
DuckDB reads what it needs, aggregates it and spills the rest.

> **The session log is the slow one, and the summary breaks at scale.** The session
> log is the only panel that has to *derive* sessions from the event table — a hash
> aggregate over every event in the window, which spills once the window holds more
> sessions than fit in the budget. At 20M rows it is ~3 s; at 100M rows it is ~24 s.
> The obvious fix is to take the most recent N events first
> (`ORDER BY created_at DESC LIMIT …`) and derive the sessions from those, instead of
> aggregating the whole window and throwing away all but 50.
>
> At **100M rows the summary query runs out of memory** and the dashboard's first
> panel fails. The memory goes into the `COUNT(DISTINCT …)` hash tables — the session
> and visitor ids are 36-character UUID strings, and there are millions of distinct
> ones in a 30-day window. The fix is to store the ids as DuckDB's 16-byte `UUID`
> type (which also makes the database ~10% smaller) rather than to approximate the
> counts. Not done yet — see the mneme issue.

The thread count matters as much as the limit — see `SUMAMI_DUCKDB_THREADS` above.
Measured on the same 5M-row table, the summary query:

| threads | time | note |
| ---: | ---: | --- |
| 1 | 291 ms | what a 1-vCPU box gets |
| 2 | 154 ms | the default for a 128 MB budget |
| 4 | 80 ms | |
| 10 | 1 713 ms | and it can run out of the 128 MB limit |

The declared limit is a **ceiling, not an allocation**: DuckDB's buffer manager held
0.5–1.8 MB while serving traffic. The RSS growth after a burst is the C allocator
keeping freed pages, not live data. `SUMAMI_DUCKDB_MEMORY_LIMIT` tunes the ceiling;
`/api/heartbeat` reports the usage, the configured limit, the limit DuckDB itself
reports back, and the thread count.

```
                    ┌───────────────────────────────┐
   browser ───────▶ │  sumami-server (one binary)    │
   (tracker.js)    │  ├─ /sumami.js   tracker       │
                    │  ├─ /api/send    collect       │
                    │  ├─ /api/*       umami compat  │
                    │  ├─ /            app shell ────┼──▶ CDN (frontend JS)
                    │  ├─ /docs/*      docs site     │
                    │  └─ ./sumami.duckdb  (data)   │
                    └───────────────────────────────┘
```

**One release artifact:**

| Artifact | What it is                                                                                   |
| -------- | -------------------------------------------------------------------------------------------- |
| `sumami` | **everything**: `sumami serve …` is the server, `sumami summary …` is the client              |

The server and the client are **the same program**: they share the DuckDB schema, the
metric definitions and the query implementation (`service/src/reports.rs`), so they cannot
disagree — and there is only **one copy of DuckDB** in the binary. Installing the binary
under another name (`sumami-server`, `sumami-client`) switches the default mode; those are
symlinks in the release, not separate builds.

Brand: **Sumami / 数猫咪** — "Sum" + "Umami", and phonetically *"Tsunami speed"*.
Slogan: *Count kitties who visit my websites. — 由猫开发*.

---

## 1. Repository map

This repository (`sumami-dev/sumami`, **public**) is a workspace of four components:

| Path       | Language       | What it is                                                                 |
| ---------- | -------------- | -------------------------------------------------------------------------- |
| `service/` | Rust           | The server library: collection API, stats API, auth, DuckDB writer, TLS, **the shared report layer**. |
| `cli/`     | Rust           | The analysis client **and the one executable** (`sumami`: server + client). |
| `js/`      | JavaScript     | The tracker (`sumami.js`) + the dashboard frontend (`sumami-app.js`). Published on a CDN. |
| `docs/`    | HTML/Markdown  | The website & documentation served by the service binary.                  |

The Rust crates `service/` and `cli/` are members of one Cargo workspace (root `Cargo.toml`).
They share the DuckDB schema through `service/src/db.rs` only by **convention**, not by code
sharing — the CLI opens the database file directly and must never write to it.

### Two-repository split (important)

| Repository            | Visibility | Contents                                                                                        |
| --------------------- | ---------- | ----------------------------------------------------------------------------------------------- |
| `sumami-dev/sumami`   | public     | This repo: code, website, docs, public design (this file). **No private material, ever.**        |
| `sumami-dev/mneme`    | private    | Decision log, agent notes, private discussion issues, unreleased plans, draft design.            |
| `sumami-dev/sumami-dev` | private  | Org-level meta / non-code assets.                                                                 |

**Rule:** anything that reveals private discussions, unreleased plans, private traffic data,
internal credentials, or personal notes belongs in `mneme` only. The public repo may *reference*
`mneme` by URL (e.g. "see the private decision log"), but must not reproduce its content.

---

## 2. Privacy & data protection (READ THIS BEFORE TOUCHING DATA CODE)

Analytics software handles data that identifies people. These rules are not stylistic —
they are **acceptance criteria**. A change that violates them does not merge.

### 2.1 The data we collect

**The baseline is Umami's field set.** We do not store less than Umami does — a feature
Umami has and we lack is a feature we owe our users. The list below is what the session
and event rows carry.

| Group | Fields |
| --- | --- |
| Page | `url`, `url_query`, `title` |
| Referrer | `referrer` (host), `referrer_path`, `referrer_query` |
| Site | `hostname` |
| Environment | `browser`, `browser_version`, `os`, `os_version`, `device`, `screen`, `language`, `country`, `region`, `city` |
| Marketing | `utm_source`, `utm_medium`, `utm_campaign`, `utm_content`, `utm_term` |
| Ad click ids | `gclid`, `fbclid`, `msclkid`, `ttclid`, `lifatid`, `twclid` |
| Performance | `lcp`, `inp`, `cls`, `fcp`, `ttfb` (Core Web Vitals) |
| Events | `event_name`, `event_type`, `tag`, `data` |
| Revenue | `revenue` per session/event, with `currency` |
| Visitors | a visitor id that survives across days |

**What we never collect** — the same short list Umami keeps:

* raw IP addresses — not stored, not logged (a hash is used to derive the visitor id)
* cookies, `localStorage` identifiers, fingerprinting (canvas, audio, fonts, hardware)
* precise geolocation (country comes from an edge header, not a GeoIP database)
* form contents, keystrokes, page text

### 2.1a The customer is a site owner

Our user is the person who **runs the site**, not the person who visits it. They care
about two things: **making money** and **what it costs**. Every design decision is
measured against those two questions:

* does this help them earn more (where did the money come from — which post, which
  referrer, which campaign, which page)?
* what does it cost them (disk, memory, CPU, operations)?

Privacy is not the selling point. *"You can see where the money came from"* is.

### 2.2 Rules that apply to every component

1. **Cookieless by default.** The tracker sets no cookies and no persistent storage
   identifiers. Honour `navigator.doNotTrack` and Global Privacy Control
   (`navigator.globalPrivacyControl`) by not sending anything.
2. **No raw IPs in logs.** Access logs record method, path, status, duration. Query strings are
   stripped from logged paths (they routinely carry session tokens and PII).
3. **No request bodies in logs.** Payloads are validated and discarded, never logged verbatim.
4. **No outbound network calls.** The service never phones home. No analytics, no crash
   reporting, no update checks.
5. **Synthetic fixtures only.** Tests, seed scripts, docs examples and screenshots use invented
   domains (`example.com`, `sumami.dev`) and invented data. Never paste real traffic into this
   repo.
6. **Retention is a feature, not an afterthought.** Events/sessions older than the configured
   retention window are prunable (`sumami prune`), and the CLI must expose the same window.
7. **Secrets never enter the repo.** JWT secret, admin password, DB path with credentials come
   from the environment or a file with `0600`. `.env` is git-ignored; `*.duckdb` and `*.duckdb.wal`
   are git-ignored.
7a. **Never trade away a feature for storage.** The engine's job is to make the full
   Umami field set cheap to keep. If a schema change would drop a field we do not have
   an equivalent for, fix the schema — not the field list.
8. **Access control is default-deny.** Statistics and website management require a valid token
   except for the deliberately public endpoints (`/api/send`, `/api/collect`, `/sumami.js`,
   `/api/heartbeat`).
9. **Vulnerability reports are private.** Ask the maintainer privately; do not open a public
   issue containing a working exploit or a real leaked value.

### 2.3 Privacy features the UI/API must expose

* `GET /api/websites/:id` returns the website's data-retention settings.
* Custom event properties are user-supplied free-form data: the documentation must warn that
  **sending PII in custom properties defeats the whole purpose**, and the tracker must not
  auto-attach anything sensitive.
* A `sumami erase` CLI command wipes one website (or everything) from the database.

---

## 3. Brand, naming and IP guardrails

* The product is **Sumami**. It is an independent open-source project.
* Compatibility is described **factually**: *"An open-source, high-performance alternative to
  Umami. Implements the Umami v2 HTTP API."* Never claim endorsement, affiliation or
  sponsorship, and never use Umami's name in a way that suggests it.
* **No Umami assets**: logo, artwork, UI screenshots, CSS, code or copy are copied. The UI
  (cats, paws, our own icons) is original work.
* Third-party names (Umami, DuckDB, Axum, Cloudflare) are referenced nominatively as
  compatible/comparable products, per the nominative-fair-use principle.
* **Marketing assets must be original or properly licensed.** Do not rip frames from
  copyrighted anime/film for promo videos; a self-drawn or clearly transformative parody is
  safe, a clip montage is not. Any third-party artwork in `docs/` needs its licence recorded
  in `docs/assets/CREDITS.md`.
* The licence is MIT (see `LICENSE`). Dependency licences must stay permissive (DuckDB: MIT,
  Axum/Tokio: MIT/Apache-2.0).

---

## 4. Architecture

### 4.1 service/ — the server

* **Runtime:** Rust 2021, Axum 0.8, Tokio (multi-thread runtime).
* **Storage:** DuckDB 2.0.0, embedded, one file. `duckdb` crate `1.20000.x`
  (the crate encodes the DuckDB version as `1.MAJOR_MINOR_PATCH.x`, so `20000` = **2.0.0**).
  DuckDB runs with an explicit `memory_limit` (default 256 MB) so a 512 MB VPS survives.
* **Static, single-file build:** DuckDB is compiled into the executable (`bundled`
  feature, the default). The release is **one self-contained binary** — copy it, run it,
  done: no shared library to ship, no `LD_LIBRARY_PATH`, no rpath, no runtime surprises.
  This is the deployment that matters: *"one file on a cheap VPS"*.
  Building with `--no-default-features` produces a much smaller executable that links
  the prebuilt `libduckdb` dynamically (then `libduckdb.so` has to ship next to it) —
  useful when the executable size matters more than the single-file property.
* **One connection, one writer:** HTTP handlers never touch the database. They push parsed
  hits into an in-memory buffer; a dedicated writer thread drains the buffer and writes one
  transaction every **2 seconds**.
  *Why one connection:* DuckDB keeps the snapshot a connection started with — a second
  connection opened *before* a write keeps returning the old state, even read-write. So the
  server uses exactly **one** connection behind a mutex: the dashboard always sees fresh
  numbers, and there is a single writer (DuckDB's model anyway).
  The CLI opens the file read-only instead, where a snapshot is exactly what an offline
  analysis wants.
* **Back-pressure / overload:** if the buffer exceeds `MAX_QUEUE` (default 100 000), the
  service drops the **oldest** buffered hits and counts them (surfaced in `/api/heartbeat`),
  instead of growing until the OOM killer arrives. Statistics do not need to be perfect; the
  server staying up matters more. The log line for it is deliberately friendly:
  `[sumami] the 33rd kitty was too cute to count, the server dozed off — later kittens can wait`.
* **Auth:** username + password (Argon2id), JWT (HS256) bearer tokens, like Umami's
  `POST /api/login` → `{ token, user }`. The admin account is created on the first run; the
  password is generated and printed **once** unless `SUMAMI_ADMIN_PASSWORD` is set.
* **TLS:** `--ssl <domain>` obtains a Let's Encrypt certificate over ACME (TLS-ALPN-01) and
  renews it in the background — no certbot, no cron, no reverse proxy needed just for TLS.
  Port 443 must reach the process directly; behind a TLS-terminating proxy, let the proxy
  manage the certificate instead.
* **Static hosting:** the binary serves `js/sumami.js` (embedded at compile time via
  `include_str!`, ~7 KB) and the docs site from `docs/` (`/docs`).
  The **dashboard frontend is not in the binary**: the shell at `/` points at a CDN
  (`SUMAMI_APP_URL`, default `https://cdn.jsdelivr.net/gh/sumami-dev/sumami@main/js`),
  which serves this public repository directly. Set `SUMAMI_APP_URL=/app` to serve it
  from the backend instead (air-gapped installs).

### 4.1b The shared query layer (`service/src/reports.rs`)

Every report — summary, timeseries, the ten breakdowns, sessions, events — has **exactly
one implementation**, in `service/src/reports.rs`:

```
service/src/reports.rs        ← the only SQL in the project
   ↑                ↑
service/api/*      cli/src/source.rs
```

The API handlers are thin wrappers around it, and the CLI's local mode calls the very same
functions. Two consequences:

* the numbers the CLI prints and the numbers the API returns are equal **by construction**;
* a metric that changes (say, how a bounce is counted) changes in one place.

Never write a second SQL query for a report that already exists here.

### 4.1c The storage trait (`service/src/storage.rs`)

Every report and every write goes through one trait, `Storage`. The open-source build
implements it once — for DuckDB, in `service/src/reports.rs`.

The trait defines the **semantics**, not the SQL: timestamps, date bucketing, distinct
counting and upsert syntax differ between engines, so each backend writes its own
dialect against the same contract. The private `sumami-pro` repository adds SQLite and
PostgreSQL backends against this trait; the open-source build ships **only DuckDB**.

Consequences for this repository:

* the API handlers, the CLI and the writer all talk to `Arc<dyn Storage>` — never to
  `duckdb::Connection` directly;
* a report has exactly one implementation, so the dashboard and the CLI cannot disagree;
* the batch grouping (`storage::aggregate_batch`) is backend-agnostic — the backends only
  differ in how they write the result.

### 4.2 Umami v2 API compatibility (the contract)

Implemented in v1 (target for the `sumami-dev/sumami` client used by the dashboard):

| Method | Path                                   | Purpose                                  |
| ------ | -------------------------------------- | ---------------------------------------- |
| POST   | `/api/send`                            | collect (`type`: event/identify/screenview) |
| POST   | `/api/collect`                         | collect (legacy query-parameter form)     |
| GET    | `/api/websites`                        | list websites                            |
| POST   | `/api/websites`                        | create website                           |
| GET    | `/api/websites/:id`                    | website detail                           |
| PUT    | `/api/websites/:id`                    | update website                           |
| DELETE | `/api/websites/:id`                    | delete website + its data                |
| GET    | `/api/websites/:id/stats`              | summary metrics                          |
| GET    | `/api/websites/:id/stats/metrics`      | metric breakdown                         |
| GET    | `/api/websites/:id/stats/timeseries`   | time series                              |
| GET    | `/api/websites/:id/stats/{pages,referrers,browsers,os,devices,countries,languages,screens,events,urls}` | breakdowns |
| GET    | `/api/websites/:id/sessions`           | session list                             |
| GET    | `/api/websites/:id/events`             | custom event list                        |
| POST   | `/api/login`, `POST /api/auth/refresh`, `GET /api/me` | auth            |
| GET    | `/api/heartbeat`                       | health + queue stats                     |

Out of scope for v1 (explicitly, so nobody "fixes" it by accident): teams, roles/permissions,
annotations, comments, share links, user management UI, plugin marketplace, realtime
(websocket), A/B testing, the Umami **dashboard front-end** (this project ships the *data
service*; the official Umami dashboard is expected to work against it, but we do not fork it —
see §6).

### 4.3 Data model: one fact table (DuckDB)

**Every page view is one row in `event`**, carrying the same fields Umami carries.
Sessions, visitors and page views are all *derived* from it — nothing is stored twice,
so nothing can drift apart, and the page attribution is exact.

```sql
event(
    id BIGINT,               -- monotonic, assigned by the writer, NOT a primary key
    website_id, session_id, visitor_id, created_at,

    url, url_query, title,                       -- the page
    referrer, referrer_path, referrer_query,     -- where it came from
    hostname,

    browser, browser_version, os, os_version, device, screen, language,
    country, region, city,

    utm_source, utm_medium, utm_campaign, utm_content, utm_term,

    gclid, fbclid, msclkid, ttclid, lifatid, twclid,   -- ad click ids

    lcp, inp, cls, fcp, ttfb,                     -- Core Web Vitals

    event_type, event_name, tag, data
)

revenue(                    -- what the owner earned, imported (not tracked)
    id BIGINT, website_id, session_id, event_id,
    event_name, currency, revenue, created_at
)
```

**The baseline is Umami.** We store everything Umami stores — plus the browser and OS
*versions*, which Umami does not keep. Two consequences worth stating plainly:

* **The visitor id survives across days** (Umami's `distinct_id`). It is derived from a
  hash of the coarse IP prefix and the user agent; the raw IP is never stored.
* **The query string is stored**, because the UTM parameters and the ad click ids live
  in it. The server does the splitting — the browser sends the whole URL.

**Why one table and not a session table**: an earlier design kept one row per session
and counted the page views in a column. It was smaller and it was **wrong**: a session's
views were all attributed to the page it happened to end on. With one row per view the
attribution is exact by construction, the write is an append (the fastest path the
engine has), and the extra rows cost a fraction of what they save.

**Why there is no index on the fact table** (schema v3): `event` and `revenue` are
append only, and every query over them is a filtered scan
(`WHERE website_id = ? AND created_at BETWEEN ?`). DuckDB prunes those with **zone
maps** — one min/max per column per row group, maintained automatically and stored
with the data — so an index buys nothing. Measured (`cargo run --release --example
index_probe -p sumami-service`):

| | with `PRIMARY KEY` | without |
| --- | ---: | ---: |
| disk, 2000000 rows | +16.3 MB (**+8.1 B/row**) | — |
| disk, the real 42-column schema, 100000 views | +1.0 MB (**+10.5 B/浏览**) | — |
| append | +9% | — |
| the dashboard query | identical plan, identical time | — |

DuckDB 2.0 **persists** the ART index of a primary key to disk, and it enforces the
uniqueness constraint on every single row. The ids are handed out by the writer
(`MAX(id) + 1`, answered from the column statistics in constant time), so the
constraint protects nothing. The small dimension tables (`website`, `meta`, `user`)
keep their keys — they hold a handful of rows, and `meta` needs its primary key for
`ON CONFLICT` to work.

**How many bytes a page view costs** (`cargo run --release --example row_cost -p
sumami-service`, the real schema, 100000 views):

| | B/浏览 |
| --- | ---: |
| synthetic traffic (a handful of pages, mostly NULL columns) | 42.1 |
| realistic traffic (long URLs, titles, UTM, click ids) | **86.6** |
| the worst case (every URL, title and referrer unique) | 209.8 |

Those numbers use the short ids the benchmark scripts use. The **server** hands out
36-character UUIDs, and the session and visitor id are stored in *every* row — that
is worth another **34 B/浏览 (42%)**, so the realistic cost of a live site is
**115.5 B/浏览**. Storing the ids as DuckDB's 16-byte `UUID` type instead of text
brings it to 105.0, and keeps the API identical (the strings are only formatted at
the edge). Not done yet — see the mneme issue.

### 4.4 cli/ — the analyzer (two sources, one UX)

The CLI (`sumami-client`, or `sumami` in its client mode) works against **either** a local
database file **or** a running service, and the subcommands are identical in both modes:

```
sumami websites                     # list websites
sumami summary   --website <id>     # pageviews, visitors, views, bounces, duration
sumami pages     --website <id>     # top paths
sumami referrers --website <id>     # traffic sources
sumami browsers  --website <id>     # browser / OS / device / country breakdowns
sumami events    --website <id>     # custom events
sumami sessions  --website <id>     # session log
sumami timeseries --website <id>    # hourly/daily series
sumami export    --website <id> --format json|csv
sumami prune     --website <id>     # enforce retention   (local only)
sumami erase     --website <id>     # delete a website's data
```

**Source selection** (exactly one of):

| Mode       | Flags                                                          | Notes |
| ---------- | -------------------------------------------------------------- | ----- |
| local file | `--db <path>` (default `./sumami.duckdb`)                       | opened `access_mode=READ_ONLY`; `prune`/`erase` open it read-write |
| HTTP API   | `--url http://host:3000` + `--token <jwt>` or `--username/--password` | logs in as **admin** via `POST /api/login`, caches the JWT in `~/.config/sumami/credentials.json` (mode `0600`) |

The **admin account** is the service's own user table (Argon2id password, JWT issued by
`POST /api/login`); it is the DB entry point for management operations. The first `serve`
on an empty database creates the admin user:

* `SUMAMI_ADMIN_USER` (default `admin`)
* `SUMAMI_ADMIN_PASSWORD` — if unset, a strong password is generated and printed **once**
  to the service log; it is never stored in plaintext.

The CLI **must never** open a local database for writing except in `prune`/`erase`.
In HTTP mode the equivalent destructive operations go through the authenticated API.

Flags common to all subcommands: `--from`, `--to` (ISO date or relative like `7d`),
`--json`, `--limit <n>` (default 50), `--no-color`.

### 4.5 js/ — the tracker and the dashboard frontend

Two independent, **dependency-free** files (no build step, no npm, no bundler):

**`js/sumami.js` — the tracker** (embedded in the binary, served at `/sumami.js`):

* sends pageviews and custom events to `/api/send` via `sendBeacon` (fallback `fetch`
  with `keepalive`), as `application/json`;
* is self-hosted by default (`<script defer src="https://sumami.example/sumami.js"
  data-website-id="...">`) — **the service serves it from its own origin**;
* sets no cookies, uses no `localStorage`, honours DNT/GPC;
* reports **Umami's full field set**: the website, the full URL and referrer, the title,
  the hostname, the language, the screen, the event name, tag and data, and the five web
  vitals (`lcp`, `inp`, `cls`, `fcp`, `ttfb` — sent as strings, the server parses them);
* sends the query string: the server parses the UTM parameters and the ad click ids
  (`gclid`, `fbclid`, `msclkid`, `ttclid`, `lifatid`, `twclid`) out of it;
* measures the web vitals with `PerformanceObserver` and sends the page view on
  `visibilitychange`/`pagehide` (the metrics are only known after the page is painted);
* patches `history.pushState`/`replaceState` and listens for `popstate`, so a single page
  application records every route change as its own page view;
* exposes `window.sumami.track(name, props)`, `window.sumami.identify(data)` and
  `window.sumami.pageview()` (with `window.umami` as an alias for existing snippets);
* is byte-for-byte reproducible: the file in the repo is the file that gets served.

**`js/sumami-app.js` + `js/sumami.css` — the dashboard frontend** (served from disk at
`/app/*`, or hosted on a CDN and pointed at with `SUMAMI_APP_URL`):

* vanilla ES2020, no framework, no chart library (the timeseries chart is hand-drawn SVG);
* logs in against `/api/login` and keeps the admin token in `localStorage` — this is the
  *dashboard's own* credential and is unrelated to the tracker, which stores nothing;
* the backend only ever serves a tiny HTML shell that references these two files, so the
  frontend can be updated (or moved to a CDN) without touching the binary.

Deployment recipes (plain `<script>`, self-hosting, CDN, multi-site) live in `js/README.md`.

### 4.5a Hot and cold: Parquet is the archive, DuckDB is the working set

The server holds an **exclusive lock** on the DuckDB file, so the database cannot be
copied while the service runs — and copying a 400 GB file is not a backup strategy
anyway. The cold tier is therefore **Parquet**, partitioned by day:

```
archive/
  sessions/day=2026-11-03/data_0.parquet
  events/day=2026-11-04/data_0.parquet
  websites.parquet
```

* `sumami backup --out ./archive` writes it — **one pass per table**
  (`COPY … TO … (FORMAT PARQUET, COMPRESSION ZSTD, PARTITION_BY (day))`).
* `sumami --parquet './archive/*' summary <id>` reads it **in place**: the archive is
  queried through `read_parquet()`, never imported, so analysing years of history costs
  nothing but the query.
* The archive is self-describing (Hive-style partitions) — DuckDB, Spark, Polars and
  pandas can all read it without knowing our layout.
* The **same report code** runs over both tiers, so the numbers are identical by
  construction. `cli/tests/archive.rs` asserts exactly that.

Rule: the hot tier holds the last N days (14–90), the cold tier holds everything older.
A 20 GB disk plus a 90-day hot window plus daily roll-ups covers ten years of history.

### 4.5b Demo data (`sumami seed`)

`sumami seed --db <file> --days 30 --visitors 140` fills a database with **synthetic**
traffic: a weighted page/referrer/country/browser distribution, returning visitors within
a day, weekend dips, and custom events. It exists so the dashboard can be evaluated and
demoed without pointing a real site at it.

Everything it generates is invented (invented domains, invented visitors). It writes
directly to the database file, so the server has to be stopped while it runs.

### 4.6 docs/ — the website

A static site (plain HTML + one stylesheet, no build step, no npm) served by the service
binary at `/docs`:

* `docs/index.html` — the landing page (Sumami branding, cats, install snippet);
* `docs/install.html`, `self-host.html`, `tracker.html`, `api.html`, `cli.html`,
  `privacy.html`, `faq.html` — the documentation;
* `docs/assets/style.css` — one stylesheet, light + dark via `prefers-color-scheme`;
* `docs/assets/CREDITS.md` — the licence record for every asset (all artwork is original).

Docs are plain HTML on purpose: no Markdown renderer has to be shipped in the binary, and
the pages work when opened straight from the filesystem.

The **dashboard** is separate from the docs: the backend serves a very small HTML shell at
`/` that loads `js/sumami-app.js` + `js/sumami.css` (see §4.5). Point `SUMAMI_APP_URL` at a
CDN to serve that frontend from anywhere; the backend binary stays tiny because the
frontend is never embedded in it.

## 5. Non-negotiable engineering rules

1. `#![forbid(unsafe_code)]` in every crate.
2. **No new runtime dependencies** without a comment in the PR explaining why. The dependency
   budget is part of the product's selling point (single binary, tiny RAM).
3. **No telemetry, no outbound requests, no analytics of our own.**
4. All user-visible strings are English (the OSS audience) and are documented in both
   `en` and `zh-CN` where the docs have a translation.
5. Every public API route has an integration test; every DuckDB query in `cli/` has a test
   against a generated fixture database.
6. `cargo test` and `cargo clippy -- -D warnings` must pass before push.
7. Schema changes are **additive** and must not require a migration step in v1
   (`CREATE TABLE IF NOT EXISTS` at open time).

---

## 6. What we are *not* building (scope guard)

* **A fork of the Umami dashboard.** We implement the API; the community dashboard is used
  as-is. Building our own dashboard is a v2+ project (and belongs in `docs/` only as an
  optional static UI).
* Anything requiring a server larger than 1 vCPU / 512 MB RAM for the base install.
* Anything requiring a managed cloud service to function.

---

## 7. Development workflow

```bash
make build            # debug build
make release          # release build (static DuckDB: self-contained binaries)
make test             # unit + integration tests
make lint             # clippy -- -D warnings
make run              # serve on :3000 with ./dev.duckdb
make package          # release artefact: the `sumami` binary + js/ + docs/
make build-small      # optional: dynamically linked DuckDB (smaller, needs libduckdb)
```

Plain cargo works too:

```bash
cargo build --release
cargo test --workspace
cargo run -p sumami-cli -- serve --db ./dev.duckdb --port 3000
cargo run -p sumami-cli -- --db ./dev.duckdb summary <website-id>
```

The first build compiles DuckDB from source — that is the slow part (set
`CARGO_HTTP_TIMEOUT=600` on a slow link).

Local smoke test:

```bash
curl -X POST http://localhost:3000/api/send \
  -H 'content-type: application/json' \
  -d '{"type":"event","payload":{"website":"<website-id>","url":"/hello"}}'
```

Note: Umami v2 sends pageviews as `type: "event"` **without** a `name`; a custom event
carries a `name`. The server treats a nameless event as a pageview.

Deployment (cheap VPS, Debian/Ubuntu):

```bash
# The release is a single self-contained binary — nothing else to install.
sudo install -m 0755 target/release/sumami /usr/local/bin/
sudo systemctl enable --now sumami
```

Optional aliases (symlinks, zero cost):
`ln -s /usr/local/bin/sumami /usr/local/bin/sumami-server`

With TLS managed by Sumami itself:

```bash
sumami-server serve --db /var/lib/sumami/sumami.duckdb \
                    --ssl example.com --ssl-email you@example.com
```

## 8. Version roadmap

* **v1 (current, implemented):** collection API, stats API, auth, DuckDB writer, CLI reader
  (local + remote), tracker, dashboard frontend, docs site, ACME TLS, one-binary deploy.
* **v1.1:** automatic retention pruning on a schedule, more breakdowns (UTM, campaigns),
  Prometheus metrics endpoint, HTTP mode for `prune`/`erase` in the CLI.
* **v2:** optional cold-tier (Parquet archive), multi-tenant teams, our own dashboard,
  distributed mode (ClickHouse) for the hosted SaaS tier.

---

## 9. Related private material

Internal decision log, unreleased plans and private discussion live in the **private**
`sumami-dev/mneme` repository (GitHub issues + notes). Agents working on Sumami may record
decisions there, but must never copy private content into this repository. When a decision
becomes public, move the *conclusion* (not the discussion) into this file.
