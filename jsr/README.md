# @sumimi/tracker

The [Sumimi](https://sumimi.dev) tracker, typed.

Sumimi is a self-hosted web analytics service: one binary, one directory, no
cookies, no fingerprints, no raw IP addresses, and an API that is compatible with
Umami v2.

The tracker itself is one file of plain JavaScript, served by the Sumami server
at `GET /sumimi.js`. This package is the typed way to use it from TypeScript.

## Install

```bash
deno add jsr:@sumimi/tracker
# or
npm install @sumimi/tracker
```

## Use

```ts
import { init } from "@sumimi/tracker";

const sumimi = init({ website: "<website-uuid>" });

// A custom event.
sumimi.track("signup", { plan: "pro" });

// Data attached to the current session.
sumimi.identify({ plan: "pro" });
```

The script is injected once and loaded once. The calls made before it is ready
are queued and sent when it is — nothing is lost.

## Options

| option | what it is |
| --- | --- |
| `website` | the website id from the dashboard, or the domain |
| `apiUrl` | the base URL of the Sumami API — the origin of the page by default |
| `scriptUrl` | the URL of the tracker script — `<apiUrl>/sumimi.js` by default |

## The script tag

If you are not using TypeScript, the script tag is all you need:

```html
<script defer src="https://analytics.example.com/sumami.js"
        data-website-id="<website-uuid>"></script>
```

## What it reports

A page view, a screen view, or a custom event — with the same fields Umami
reports. Do Not Track and Global Privacy Control are honoured: if either says
"no", nothing is sent at all.

## Licence

Apache-2.0.
