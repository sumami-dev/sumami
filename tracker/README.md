# The Sumimi tracker

`@sumimi/tracker` on npm · `@ljh/sumimi` on JSR

The [Sumimi](https://sumimi.dev) tracker, typed.

Sumimi is a self-hosted web analytics service: one binary, one directory, no
cookies, no fingerprints, no raw IP addresses, and an API that is compatible with
Umami v2.

The tracker itself is one file of plain JavaScript, served by the Sumimi server
at `GET /sumimi.js`. This package is the typed way to use it from TypeScript.

## Install

```bash
npm install @sumimi/tracker
# or
deno add jsr:@ljh/sumimi
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
| `apiUrl` | the base URL of the Sumimi API — the origin of the page by default |
| `scriptUrl` | the URL of the tracker script — `<apiUrl>/sumimi.js` by default |

## The script tag

If you are not using TypeScript, the script tag is all you need:

```html
<script defer src="https://analytics.example.com/sumimi.js"
        data-website-id="<website-uuid>"></script>
```

## What it reports

A page view, a screen view, or a custom event — with the same fields Umami
reports. Do Not Track and Global Privacy Control are honoured: if either says
"no", nothing is sent at all.

## The package

One source, two registries:

| file | what it is |
| --- | --- |
| `src/mod.ts` | the source — the only copy of the code |
| `jsr.json` | the JSR package (`@ljh/sumimi` on jsr.io) |
| `package.json` | the npm package (`@sumimi/tracker` on npmjs.com) |
| `tsconfig.json` | the npm build (`src/mod.ts` → `dist/`) |

```bash
deno task test              # the tests
deno publish --dry-run      # what JSR would publish
npm run build && npm pack   # what npm would publish
```

## Licence

Apache-2.0.
