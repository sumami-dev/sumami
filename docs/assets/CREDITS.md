# Credits and licences — `docs/`

This file is the licence record for everything in the `docs/` directory of the
Sumami repository, as required by the project's design document (AGENTS.md §3).

## Summary

Everything shipped in `docs/` is **original work** created for the Sumami project
and is covered by the project's own MIT licence (`LICENSE` at the root of the
repository). No third-party artwork, photographs, icon sets, fonts, CSS
frameworks, JavaScript libraries or tracking scripts are bundled here.

The documentation site is deliberately dependency-free: it uses no build step,
no npm, no bundler, no CSS framework and no web fonts. Every page is plain
static HTML plus one shared stylesheet, so the site loads fast and works
completely offline.

## Files in this directory

| File | What it is | Licence |
| ---- | ---------- | ------- |
| `style.css` | The single shared stylesheet for every documentation page. Hand-written; light and dark themes via `prefers-color-scheme`; responsive layout. | MIT (original work) |
| `CREDITS.md` | This file. | MIT (original work) |

The HTML pages live one directory up, in `docs/` itself:

| File | What it is | Licence |
| ---- | ---------- | ------- |
| `index.html` | Landing page for sumami.dev. | MIT (original work) |
| `install.html` | Installation and quick start. | MIT (original work) |
| `self-host.html` | Self-hosting on a cheap VPS: systemd, reverse proxy, TLS/ACME, backups. | MIT (original work) |
| `tracker.html` | Tracker installation and deployment options. | MIT (original work) |
| `api.html` | HTTP API reference. | MIT (original work) |
| `cli.html` | `sumami` CLI reference. | MIT (original work) |
| `privacy.html` | Privacy and data-protection page. | MIT (original work) |
| `faq.html` | Frequently asked questions. | MIT (original work) |

## Artwork

**All artwork in `docs/` is original.**

The paw prints and the cat-and-chart illustration are hand-written inline SVG
embedded directly in the HTML markup. They were drawn for Sumami as simple
geometric shapes (ellipses and paths). No character, sprite, photograph,
illustration, logo or icon from any existing franchise, anime, film, game,
cartoon or other copyrighted work was used, traced, sampled or adapted.

The only other graphic element is the favicon, which is an inline SVG data URI
containing a single text glyph of a paw sign in the page's own `<head>`. It
requires no additional file and no external asset.

The user-interface icons used in the pages (shield, database, terminal, clock,
list, download, checkmark and similar) are likewise hand-written inline SVG
built from basic strokes and paths.

## Fonts

**No fonts are bundled, downloaded, embedded or referenced.**

The site uses **system font stacks** only, declared in `docs/assets/style.css`:

```css
--font-sans: ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto,
  "Helvetica Neue", Arial, "Noto Sans", "PingFang SC", "Hiragino Sans GB",
  "Microsoft YaHei", sans-serif;
--font-mono: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas,
  "Liberation Mono", "Courier New", monospace;
```

These resolve to fonts already installed on the visitor's own device. Because
no font file is fetched, the site makes no third-party requests and works with
no network access at all. The named families are only fallbacks after the
generic `ui-*`/`system-ui` keywords and the generic `sans-serif`/`monospace`
families; none of them is redistributed here, so their licences do not apply to
this repository.

## Third-party software referenced (not bundled)

The documentation *names* the following third-party projects, because Sumami is
built on or compatible with them. None of their code, binaries, assets or
artwork is included in `docs/`. The names are used nominatively, to identify
compatible and comparable products.

| Project | Role | Licence |
| ------- | ---- | ------- |
| [DuckDB](https://github.com/duckdb/duckdb) | The embedded analytics database. Version 2.0.0. The `libduckdb.so` / `libduckdb.dylib` shared library is distributed in the **release archive**, not in `docs/`. | MIT |
| [Axum](https://github.com/tokio-rs/axum) | The HTTP framework and router used by the `sumami-server` service. | MIT |
| [Tokio](https://github.com/tokio-rs/tokio) | The asynchronous runtime used by the service. | MIT / Apache-2.0 |
| [Umami](https://github.com/umami-software/umami) | The analytics product whose **v2 HTTP API** Sumami implements for compatibility. The official Umami dashboard is expected to work against a Sumami server. | MIT (Umami itself; no Umami assets are used in `docs/`) |
| [Caddy](https://github.com/caddyserver/caddy) | An optional reverse proxy mentioned in the self-hosting guide. Not bundled. | Apache-2.0 |
| [nginx](https://nginx.org/) | An optional reverse proxy mentioned in the self-hosting guide. Not bundled. | 2-clause BSD |
| [Let's Encrypt](https://letsencrypt.org/) | The certificate authority whose ACME certificates are described in the self-hosting guide. Not bundled. | Public domain / MPL-2.0 (ACME client libraries vary) |
| [certbot](https://github.com/certbot/certbot) | An optional ACME client mentioned in the self-hosting guide. Not bundled. | Apache-2.0 |
| [Cloudflare](https://www.cloudflare.com/) | Mentioned only as an example of a CDN that can supply a country-code header. Not required, not bundled, not affiliated. | Proprietary service |

## Trademarks and naming

"Sumami" and "数猫咪" are the names of this project. Third-party names —
Umami, DuckDB, Axum, Tokio, Caddy, nginx, Let's Encrypt, Cloudflare — are
referenced nominatively, in accordance with the nominative-fair-use principle,
to identify compatible, comparable or recommended products. No claim of
endorsement, affiliation or sponsorship is made or implied.

Sumami is an independent open-source project. It is described factually as
*an open-source alternative to Umami that implements the Umami v2 HTTP API*.

## Any future additions

If third-party artwork, photographs, icon sets, web fonts or other media are
ever added to `docs/`, that asset must be listed in this file with its source
URL, its author and its licence before it is committed. The design document
(AGENTS.md §3) requires that marketing and documentation assets are either
original or properly licensed, and that the licence of every asset in `docs/`
is recorded here.
