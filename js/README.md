# `js/` — the tracker and the dashboard frontend

Two independent, dependency-free files. No build step, no npm, no bundler: the file in
this directory is the file that is served.

| File             | What it is                                                    | Served at         |
| ---------------- | ------------------------------------------------------------- | ----------------- |
| `sumami.js`      | The tracker you put on **your own website**.                   | `/sumami.js`     |
| `sumami-app.js`  | The Sumami **dashboard** frontend.                            | `/app/sumami-app.js` |
| `sumami.css`     | The dashboard stylesheet.                                      | `/app/sumami.css`   |

---

## `sumami.js` — the tracker

### Install

```html
<script defer src="https://analytics.example.com/sumami.js"
        data-website-id="0f8fad5b-d9cb-469f-a165-70867728950e"></script>
```

That is the whole installation. The tracker is served by your own Sumami instance, so
there is no third party in the request path.

The tracker is one dependency-free file of plain ES5 JavaScript — no build step, no
npm, no bundler. It reports Umami's full field set, including the Core Web Vitals
(`lcp`, `inp`, `cls`, `fcp`, `ttfb`), and it works with single page applications.

### Attributes

| Attribute          | Meaning                                                        | Default        |
| ------------------ | -------------------------------------------------------------- | -------------- |
| `data-website-id`  | The website id (UUID). Required unless `data-website` is given.  | —              |
| `data-website`     | The website domain, instead of an id.                           | —              |
| `data-api-url`     | Where to send the hits.                                          | the script's own origin |

There is no opt-in for the query string: it is always sent, because the server parses
the marketing parameters out of it. See [the query string](#the-query-string) below.

### Custom events

```html
<script>
  sumami.track('signup', { plan: 'pro' });        // a custom event
  sumami.track('click', { button: 'buy' }, 'cta'); // with a tag
  sumami.identify({ plan: 'pro' });               // attaches data to the current session
  sumami.pageview();                              // a manual page view
</script>
```

`window.umami` is an alias for `window.sumami`, so existing Umami snippets keep working.

### What it sends

Every page view is one row with Umami's field set:

```json
{
  "type": "event",
  "payload": {
    "website": "0f8fad5b-d9cb-469f-a165-70867728950e",
    "url": "/blog/post?utm_source=newsletter&utm_campaign=spring",
    "referrer": "https://news.ycombinator.com/item?id=1",
    "title": "My post",
    "hostname": "example.com",
    "language": "en-US",
    "screen": "1920x1080",
    "lcp": "1500.5",
    "inp": "120",
    "cls": "0.05",
    "fcp": "800",
    "ttfb": "123.5"
  }
}
```

| Field      | What it is                                                        |
| ---------- | ----------------------------------------------------------------- |
| `website`  | The id or the domain you configured. The only required field.       |
| `url`      | The **full** current URL: the path *and* the query string.         |
| `referrer` | `document.referrer`, in full. An empty string when there is none.   |
| `title`    | `document.title`.                                                  |
| `hostname` | `location.hostname`.                                               |
| `language` | `navigator.language`.                                              |
| `screen`   | `<width>x<height>` of the screen (not the viewport).                |
| `name`     | The custom event name. Only for `track()`.                          |
| `tag`      | The custom event tag. Only for `track()`.                           |
| `data`     | The custom event payload, as given.                                 |
| `lcp`      | Largest Contentful Paint, in milliseconds.                          |
| `inp`      | Interaction to Next Paint, in milliseconds.                         |
| `cls`      | Cumulative Layout Shift, as a ratio.                                |
| `fcp`      | First Contentful Paint, in milliseconds.                            |
| `ttfb`     | Time to First Byte, in milliseconds.                                |

The five metrics are sent as strings — the server parses them itself. Every other
field is sent exactly as the browser reported it.

The **browser family, browser version, operating system, OS version and device class
are derived server-side** from the `User-Agent` header. Nothing is fingerprinted in
the browser: no canvas, no audio, no font list, no hardware probing.

### The web vitals

The tracker measures the Core Web Vitals with `PerformanceObserver`:

| Metric | Source                                                    |
| ------ | --------------------------------------------------------- |
| `lcp`  | `largest-contentful-paint` — the last candidate            |
| `fcp`  | `paint` — the `first-contentful-paint` entry               |
| `cls`  | `layout-shift` — the worst session window (see below)      |
| `inp`  | `event` with a `durationThreshold` — the longest interaction |
| `ttfb` | `PerformanceNavigationTiming.responseStart`                |

`cls` is the *worst session window*, as the specification defines it: shifts that
follow each other closely are summed into one window, and the worst window is the
score. Shifts that follow a user interaction (`hadRecentInput`) are ignored.

The metrics are only known once the page has been painted, so the page view is sent
when the document becomes hidden (`visibilitychange`) or the page is closed
(`pagehide`) — with `navigator.sendBeacon`, so it survives the unload. A page view is
never sent twice.

### Single page applications

The tracker patches `history.pushState` and `history.replaceState` and listens for
`popstate`, so every route change of an SPA is recorded as its own page view.

**Both sides of a navigation are counted.** When you leave a route, that route's
page view is sent straight away — with the metrics it had — and the route you
arrive at is sent when you leave *it*. So a session that visits three routes
reports three page views, and the landing page is never lost:

```
/            ← the landing page, sent when the first navigation happens
/route-1     ← sent when the second navigation happens
/route-2     ← sent when the tab is closed
```

A page view is never sent twice: the tracker keeps one flag per page, so a page
that has been reported is not reported again — not on the next `pagehide`, not on
the next `visibilitychange`.

**The metrics belong to the document, not to the route.** A single page
application that changes its route does not load a new document — it keeps the
one it loaded, and the Core Web Vitals keep describing *that* document. So every
page view of a document reports the **same** document-level numbers: they are
measured once and frozen, and a route change reports the document's metrics, not
a partial set of its own.

Without this, a visitor who browses three routes of an SPA would report the same
document's LCP three times with three different values, and every average would
be wrong.

### The query string

The query string **is** sent, in full, and so is the full referrer. The server does
the splitting: it separates the path from the query, and it parses the marketing
parameters out of the query string:

`utm_source`, `utm_medium`, `utm_campaign`, `utm_content`, `utm_term`, and the ad
click ids `gclid`, `fbclid`, `msclkid`, `ttclid`, `lifatid`, `twclid`.

The referrer is split the same way, into its host, its path and its query.

### What it does **not** do

* no cookies, no `localStorage`, no fingerprinting;
* nothing at all when the visitor has Do Not Track or Global Privacy Control enabled;
* no third-party requests — the hits go to your own server;
* it never throws: a failure in the tracker cannot break the page it is on.

### Deployment recipes

**1. Self-hosted (recommended)** — the tracker is served by your Sumami instance at
`/sumami.js`, from the same origin as the API. Nothing else to do, and no
cross-origin request is ever made.

```html
<script defer src="https://analytics.example.com/sumami.js"
        data-website-id="0f8fad5b-d9cb-469f-a165-70867728950e"></script>
```

**2. From your own CDN** — copy `sumami.js` into your static bucket and point the
`data-api-url` attribute at your server:

```html
<script defer src="https://cdn.example.com/sumami.js"
        data-api-url="https://analytics.example.com"
        data-website-id="0f8fad5b-d9cb-469f-a165-70867728950e"></script>
```

**3. Several websites, one instance** — create one website per domain and use its id.
The domain is optional, but it makes the dashboard easier to read.

```html
<script defer src="https://analytics.example.com/sumami.js"
        data-website="blog.example.com"></script>
```

**4. Behind a reverse proxy** — nothing special; the proxy only needs to pass
`POST /api/send` through. Do not cache `/api/send` (the service already sends
`Cache-Control: public, max-age=300` for the tracker file itself).

**A note on cross-origin deployments.** The tracker posts `application/json`, which
browsers only allow across origins after a CORS preflight. If you serve the tracker
from a different origin than your API (recipes 2 and 3), your server must answer the
`OPTIONS` preflight for `POST /api/send`. Serving the tracker from the Sumami
instance itself (recipe 1) needs no CORS setup at all.

---

## `sumami-app.js` — the dashboard frontend

The backend serves a very small HTML document at `/`:

```html
<link rel="stylesheet" href="/app/sumami.css">
<script src="/app/sumami-app.js" defer></script>
```

Both files are served from this directory at runtime. To serve them from a CDN instead,
set `SUMAMI_APP_URL` to the CDN base URL and make sure the CDN serves them with the same
paths (`<base>/sumami-app.js`, `<base>/sumami.css`):

```bash
SUMAMI_APP_URL=https://cdn.example.com/sumami \
  sumami-server serve --db /var/lib/sumami/sumami.duckdb
```

The frontend talks to the API on its own origin by default. When it is hosted on a CDN,
tell it where the API is — either with a query parameter on the dashboard URL
(`https://analytics.example.com/?api=https://api.example.com`) or with a meta tag:

```html
<meta name="sumami-api-url" content="https://api.example.com">
```

The frontend stores the **admin token** in `localStorage` under `sumami.token` so a reload
keeps you logged in. That is the dashboard's own credential — completely separate from
the tracker, which stores nothing at all.
