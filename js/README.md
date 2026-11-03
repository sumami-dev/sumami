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

### Attributes

| Attribute          | Meaning                                                        | Default        |
| ------------------ | -------------------------------------------------------------- | -------------- |
| `data-website-id`  | The website id (UUID). Required unless `data-website` is given.  | —              |
| `data-website`     | The website domain, instead of an id.                           | —              |
| `data-api-url`     | Where to send the hits.                                          | the script's own origin |
| `data-allow-query` | Send the query string of the page URL (`"true"`).                | off            |

### Custom events

```html
<script>
  sumami.track('signup', { plan: 'pro' });
  sumami.identify({ plan: 'pro' });   // attaches data to the current session
</script>
```

`window.umami` is an alias for `window.sumami`, so existing Umami snippets keep working.

### What it sends

```json
{
  "type": "event",
  "payload": {
    "website": "0f8fad5b-d9cb-469f-a165-70867728950e",
    "url": "/blog/post",
    "hostname": "example.com",
    "referrer": "https://news.ycombinator.com/",
    "title": "My post",
    "language": "en-US",
    "screen": "1920x1080"
  }
}
```

### What it does **not** do

* no cookies, no `localStorage`, no fingerprinting;
* no query string unless `data-allow-query` is set;
* nothing at all when the visitor has Do Not Track or Global Privacy Control enabled;
* no third-party requests — the hits go to your own server.

### Deployment recipes

**1. Self-hosted (recommended)** — the tracker is served by your Sumami instance at
`/sumami.js`. Nothing else to do.

**2. From your own CDN** — copy `sumami.js` into your static bucket and point the
`data-api-url` attribute at your server:

```html
<script defer src="https://cdn.example.com/sumami.js"
        data-api-url="https://analytics.example.com"
        data-website-id="0f8fad5b-d9cb-469f-a165-70867728950e"></script>
```

**3. Several websites, one instance** — create one website per domain and use its id.
The domain is optional, but it makes the dashboard easier to read.

**4. Behind a reverse proxy** — nothing special; the proxy only needs to pass
`POST /api/send` through. Do not cache `/api/send` (the service already sends
`Cache-Control: public, max-age=300` for the tracker file itself).

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
