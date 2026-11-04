/*!
 * Sumimi tracker — one file, no dependencies, no build step.
 *
 * What this file is
 * -----------------
 * A privacy-first web analytics tracker with the same field set as Umami v2.
 * It is plain ES5 JavaScript (no bundler, no npm, no polyfills) and it is served
 * by the Sumimi backend itself at `GET /sumimi.js` — the file in this repository
 * is byte-for-byte the file that gets served.
 *
 * What it reports
 * ---------------
 * Every page view is stored as one row with Umami's field set:
 *
 *   website   the website id (data-website-id) or domain (data-website)
 *   url       the full current URL: pathname + search (the server splits it)
 *   referrer  document.referrer, the full URL (the server splits it)
 *   title     document.title
 *   hostname  location.hostname
 *   language  navigator.language
 *   screen    "<width>x<height>" of the screen
 *   name      the custom event name
 *   tag       an optional custom event tag
 *   data      the custom event payload (object or string)
 *   lcp       Largest Contentful Paint, in milliseconds
 *   inp       Interaction to Next Paint, in milliseconds
 *   cls       Cumulative Layout Shift, as a ratio
 *   fcp       First Contentful Paint, in milliseconds
 *   ttfb      Time to First Byte, in milliseconds
 *
 * The five metrics are sent as strings (the server parses them itself); every
 * other field is sent as the browser reported it.
 *
 * The web vitals are collected with PerformanceObserver and are sent together
 * with the page view, once the page has been painted.
 *
 * Privacy
 * -------
 *   - no cookies, no localStorage, no fingerprinting
 *   - Do Not Track and Global Privacy Control are honoured: nothing is sent
 *   - the query string IS sent, because the server parses the UTM parameters
 *     and the ad click ids (gclid, fbclid, msclkid, ttclid, lifatid, twclid)
 *     out of it. The referrer and the page URL are sent in full for the same
 *     reason — the server, not the browser, does the splitting.
 *
 * Usage
 * -----
 *   <script defer src="https://sumimi.example/sumimi.js"
 *           data-website-id="<website-uuid>"></script>
 *
 * Optional attributes:
 *   data-website-id   the website id (required unless data-website is given)
 *   data-website      the website domain, instead of an id
 *   data-api-url      the API base URL (default: the script's own origin)
 *
 * Public API (window.sumimi, aliased as window.umami):
 *   sumimi.track(name, data)   a custom event
 *   sumimi.identify(data)      attach data to the current session
 *   sumimi.pageview()          a manual page view
 */
(function () {
  'use strict';

  var script = document.currentScript;
  if (!script) return;

  var WEBSITE_ID = script.getAttribute('data-website-id') || '';
  var WEBSITE = script.getAttribute('data-website') || '';
  var API_URL = (script.getAttribute('data-api-url') || '').replace(/\/$/, '');

  if (!WEBSITE_ID && !WEBSITE) return;

  // --- privacy: honour Do Not Track and Global Privacy Control ---
  var navigatorRef = window.navigator || {};
  var dnt = navigatorRef.doNotTrack || window.doNotTrack;
  if (dnt === '1' || dnt === 'yes' || navigatorRef.globalPrivacyControl === true) {
    return;
  }

  if (!API_URL) {
    // Derive the API origin from the script URL: <origin>/sumimi.js -> <origin>
    var src = script.src || '';
    var match = src.match(/^(https?:\/\/[^\/]+)/);
    API_URL = match ? match[1] : '';
  }
  if (!API_URL) return;

  /** The whole tracker must never break the host page. */
  function guard(fn) {
    return function () {
      try {
        return fn.apply(this, arguments);
      } catch (e) {
        return undefined;
      }
    };
  }

  // ---------------------------------------------------------------------------
  // The web vitals
  // ---------------------------------------------------------------------------

  var vitals = {
    lcp: 0, // Largest Contentful Paint, ms
    inp: 0, // Interaction to Next Paint, ms
    cls: 0, // Cumulative Layout Shift
    fcp: 0, // First Contentful Paint, ms
    ttfb: 0 // Time to First Byte, ms
  };

  /** The current layout-shift session window and its accumulated score. */
  var clsSession = { value: 0, entries: [] };

  /**
   * The CLS of a page is the worst *session window*: a burst of shifts counts
   * as one, and the gaps between two bursts are ignored.
   */
  function updateCls() {
    var entries = clsSession.entries;
    var max = 0;
    var sessionValue = 0;
    var sessionEntries = [];

    for (var i = 0; i < entries.length; i++) {
      var entry = entries[i];
      var first = sessionEntries.length === 0;
      var last = sessionEntries[sessionEntries.length - 1];

      if (
        !first &&
        entry.startTime - last.startTime < 1000 &&
        entry.startTime - last.startTime < 5000
      ) {
        // Still the same session window.
      } else {
        // A new window starts: the previous one is final.
        if (sessionValue > max) max = sessionValue;
        sessionValue = 0;
        sessionEntries = [];
      }

      sessionValue += entry.value;
      sessionEntries.push(entry);
    }

    if (sessionValue > max) max = sessionValue;

    clsSession.value = max;
    vitals.cls = max;
  }

  /** Observe one performance entry type and hand it to a callback. */
  function observe(type, callback) {
    if (typeof PerformanceObserver === 'undefined') return;
    if (!PerformanceObserver.supportedEntryTypes) return;
    if (PerformanceObserver.supportedEntryTypes.indexOf(type) === -1) return;

    try {
      var observer = new PerformanceObserver(function (list) {
        var entries = list.getEntries();
        for (var i = 0; i < entries.length; i++) callback(entries[i]);
      });
      observer.observe({ type: type, buffered: true });
    } catch (e) {
      /* an unsupported option — ignore the metric */
    }
  }

  /** Largest Contentful Paint: the last candidate wins. */
  observe('largest-contentful-paint', function (entry) {
    var value = entry.renderTime || entry.loadTime || entry.startTime;
    if (value > vitals.lcp) vitals.lcp = value;
  });

  /** First Contentful Paint: the first paint entry with the right name. */
  observe('paint', function (entry) {
    if (entry.name === 'first-contentful-paint' && entry.startTime > 0) {
      vitals.fcp = entry.startTime;
    }
  });

  /** Layout shifts: ignore the ones that follow a user interaction. */
  observe('layout-shift', function (entry) {
    if (entry.hadRecentInput) return;
    clsSession.entries.push({ value: entry.value, startTime: entry.startTime });
    updateCls();
  });

  /**
   * Interaction to Next Paint: the longest interaction of the page.
   * `durationThreshold` keeps the observer from reporting every tiny event.
   */
  try {
    if (typeof PerformanceObserver !== 'undefined') {
      var eventObserver = new PerformanceObserver(function (list) {
        var entries = list.getEntries();
        for (var i = 0; i < entries.length; i++) {
          var duration = entries[i].duration || 0;
          if (duration > vitals.inp) vitals.inp = duration;
        }
      });
      eventObserver.observe({ type: 'event', durationThreshold: 40, buffered: true });
    }
  } catch (e) {
    /* INP is not available here */
  }

  /** Time to First Byte: the navigation timing's response start. */
  function readTtfb() {
    try {
      var nav = window.performance && window.performance.getEntriesByType
        ? window.performance.getEntriesByType('navigation')[0]
        : null;
      if (nav && nav.responseStart > 0) {
        vitals.ttfb = nav.responseStart;
        return;
      }
      if (nav && nav.responseStart === 0 && nav.responseEnd > 0) {
        // A cached response: the browser still reports a response start.
        vitals.ttfb = nav.responseEnd;
        return;
      }
      var timing = window.performance && window.performance.timing;
      if (timing && timing.responseStart > 0) {
        vitals.ttfb = timing.responseStart - timing.navigationStart;
      }
    } catch (e) {
      /* no navigation timing */
    }
  }
  readTtfb();

  // ---------------------------------------------------------------------------
  // The payload
  // ---------------------------------------------------------------------------

  /** The full current URL: the server splits the path from the query. */
  function currentUrl() {
    var loc = window.location;
    var url = loc.pathname || '/';
    if (loc.search) url += loc.search;
    return url;
  }

  /** The referring page, in full. */
  function currentReferrer() {
    return document.referrer || '';
  }

  /** "<width>x<height>" of the screen. */
  function screenSize() {
    var scr = window.screen;
    if (!scr) return '';
    return scr.width + 'x' + scr.height;
  }

  /**
   * The vitals of **this document**, frozen the first time they are reported.
   *
   * The Core Web Vitals describe a *document*, not a route: LCP is the largest
   * paint of the page, CLS is the layout shift of the page, INP is the longest
   * interaction of the page. A single page application that changes its route
   * does not get a new document — it keeps the one it loaded.
   *
   * So every page view of a document reports the **same** document-level
   * numbers. Without this, a visitor who browses three routes of an SPA would
   * report the same document's LCP three times, with three different values,
   * and every average would be wrong.
   */
  var documentVitals = null;

  /** The document-level vitals, frozen the first time they are used. */
  function documentLevelVitals() {
    if (!documentVitals) {
      documentVitals = {
        lcp: vitals.lcp,
        inp: vitals.inp,
        cls: vitals.cls,
        fcp: vitals.fcp,
        ttfb: vitals.ttfb
      };
    }
    return documentVitals;
  }

  /**
   * The fields every hit carries.
   *
   * `withVitals` is false for the custom events: the web vitals describe the
   * document, and they are sent with the page views.
   */
  function basePayload(withVitals) {
    var payload = {
      website: WEBSITE_ID || WEBSITE,
      url: currentUrl(),
      referrer: currentReferrer(),
      title: document.title,
      hostname: window.location.hostname,
      language: navigatorRef.language || '',
      screen: screenSize()
    };

    if (withVitals) {
      // The server reads the metrics as strings and parses them itself
      // (`raw.str(name).parse::<f64>()`), so they are sent as strings.
      var metrics = documentLevelVitals();
      payload.lcp = String(metrics.lcp);
      payload.inp = String(metrics.inp);
      payload.cls = String(metrics.cls);
      payload.fcp = String(metrics.fcp);
      payload.ttfb = String(metrics.ttfb);
    }

    return payload;
  }

  // ---------------------------------------------------------------------------
  // Sending
  // ---------------------------------------------------------------------------

  /**
   * Send one hit.
   *
   * @param {string} type    'event' | 'identify'
   * @param {object} payload
   */
  function send(type, payload) {
    var body;
    try {
      body = JSON.stringify({ type: type, payload: payload });
    } catch (e) {
      return;
    }

    var url = API_URL + '/api/send';

    // `sendBeacon` survives the page being unloaded — it is the only way to
    // get a page view out of a tab that is being closed.
    if (navigatorRef.sendBeacon) {
      try {
        var blob = new Blob([body], { type: 'application/json' });
        if (navigatorRef.sendBeacon(url, blob)) return;
      } catch (e) {
        /* fall through to fetch */
      }
    }

    try {
      if (window.fetch) {
        window.fetch(url, {
          method: 'POST',
          body: body,
          headers: { 'Content-Type': 'application/json' },
          keepalive: true,
          mode: 'cors',
          credentials: 'omit'
        }).catch(function () {});
      }
    } catch (e) {
      /* never break the host page */
    }
  }

  // ---------------------------------------------------------------------------
  // The public API
  // ---------------------------------------------------------------------------

  /**
   * Track a custom event.
   * @param {string} name
   * @param {object|string} [data]
   * @param {string} [tag]
   */
  function track(name, data, tag) {
    if (!name) return;
    var payload = basePayload(false);
    payload.name = name;
    if (tag) payload.tag = tag;
    if (data !== undefined && data !== null) payload.data = data;
    send('event', payload);
  }

  /**
   * Attach data to the current session.
   * @param {object} data
   */
  function identify(data) {
    var payload = basePayload(false);
    payload.data = data || {};
    send('identify', payload);
  }

  /**
   * Track a page view by hand (the tracker sends one on its own).
   *
   * @param {string} [url] the URL to report, when it is not the current one
   */
  function pageview(url) {
    var payload = basePayload(true);
    if (url) payload.url = url;
    send('event', payload);
  }

  // The public API. `umami` is an alias for compatibility with existing snippets.
  var api = { track: guard(track), identify: guard(identify), pageview: guard(pageview) };
  window.sumimi = api;
  window.umami = api;

  // ---------------------------------------------------------------------------
  // When to send the page view
  // ---------------------------------------------------------------------------

  // The web vitals are only known after the page has been painted, so the page
  // view is sent when the document goes away — with `sendBeacon`, so it
  // survives the unload.
  var sent = false;

  /** The URL of the page we are on. Tracked across single page navigations. */
  var lastUrl = currentUrl();

  /** Send the page view of the current page, once. */
  function sendPageview() {
    if (sent) return;
    sent = true;
    readTtfb();
    pageview();
  }

  if (document.addEventListener) {
    document.addEventListener('visibilitychange', function () {
      if (document.visibilityState === 'hidden') sendPageview();
    });
    window.addEventListener('pagehide', sendPageview);
  }

  // ---------------------------------------------------------------------------
  // Single page applications
  // ---------------------------------------------------------------------------

  // A history navigation ends one page and starts another, so **both** get a page
  // view: the page we are leaving is reported now (with the metrics it had), and
  // the page we arrive at is reported when we leave *it*.
  //
  // Reporting only the arriving route would lose the landing page of every
  // single page application — the entry page, which is the one a site owner
  // cares most about.
  function onNavigation() {
    if (!sent) {
      sent = true;
      readTtfb();
      pageview(lastUrl);
    }
    lastUrl = currentUrl();
    sent = false;
  }

  if (window.history && typeof window.history.pushState === 'function') {
    var pushState = window.history.pushState;
    var replaceState = window.history.replaceState;

    window.history.pushState = function () {
      var result = pushState.apply(this, arguments);
      onNavigation();
      return result;
    };
    window.history.replaceState = function () {
      var result = replaceState.apply(this, arguments);
      onNavigation();
      return result;
    };

    window.addEventListener('popstate', onNavigation);
  }
})();
