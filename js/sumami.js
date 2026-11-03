/*!
 * Sumami tracker — a single, dependency-free file.
 *
 * Privacy defaults (see AGENTS.md §2):
 *   - no cookies, no localStorage, no fingerprinting
 *   - the query string is never sent (opt in with data-allow-query)
 *   - DNT / GPC are honoured: nothing is sent when the visitor opted out
 *
 * Usage:
 *   <script defer src="https://sumami.example/sumami.js"
 *           data-website-id="<website-uuid>"></script>
 *
 * Optional attributes:
 *   data-website-id   the website id (required unless data-website is given)
 *   data-website      the website domain, instead of an id
 *   data-api-url      the API base URL (default: the script's own origin)
 *   data-allow-query  set to "true" to include the query string in the URL
 *   data-collect-pii  never used; custom event properties are sent as given
 */
(function () {
  'use strict';

  var script = document.currentScript;
  if (!script) return;

  var WEBSITE_ID = script.getAttribute('data-website-id') || '';
  var WEBSITE = script.getAttribute('data-website') || '';
  var API_URL = (script.getAttribute('data-api-url') || '').replace(/\/$/, '');
  var ALLOW_QUERY = script.getAttribute('data-allow-query') === 'true';

  if (!WEBSITE_ID && !WEBSITE) return;

  // --- privacy: honour Do Not Track and Global Privacy Control ---
  var navigatorRef = window.navigator || {};
  var dnt = navigatorRef.doNotTrack || window.doNotTrack;
  if (dnt === '1' || dnt === 'yes' || navigatorRef.globalPrivacyControl === true) {
    return;
  }

  if (!API_URL) {
    // Derive the API origin from the script URL: <origin>/sumami.js -> <origin>
    var current = document.currentScript;
    var src = current && current.src ? current.src : '';
    var match = src.match(/^(https?:\/\/[^\/]+)/);
    API_URL = match ? match[1] : '';
  }
  if (!API_URL) return;

  var screen = window.screen ? window.screen.width + 'x' + window.screen.height : '';
  var language = navigatorRef.language || '';

  /** Strip the query string and fragment unless the site opted in. */
  function cleanUrl(url) {
    if (ALLOW_QUERY) return url;
    var hash = url.indexOf('#');
    var withoutHash = hash === -1 ? url : url.slice(0, hash);
    var query = withoutHash.indexOf('?');
    return query === -1 ? withoutHash : withoutHash.slice(0, query);
  }

  /** Current page path (and query, if allowed). */
  function currentUrl() {
    var url = window.location.pathname || '/';
    if (ALLOW_QUERY && window.location.search) url += window.location.search;
    return url;
  }

  /** The referring page, cleaned the same way. */
  function referrer() {
    var ref = document.referrer;
    if (!ref) return undefined;
    try {
      var parsed = new URL(ref, window.location.href);
      return cleanUrl(parsed.origin === window.location.origin ? parsed.pathname : parsed.href);
    } catch (e) {
      return undefined;
    }
  }

  /**
   * Send one hit.
   * @param {string} type  'event' | 'screenview' | 'identify'
   * @param {object} payload
   */
  function send(type, payload) {
    var body = JSON.stringify({
      type: type,
      payload: payload
    });

    var url = API_URL + '/api/send';

    if (navigatorRef.sendBeacon) {
      try {
        var blob = new Blob([body], { type: 'text/plain;charset=UTF-8' });
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
          headers: { 'Content-Type': 'text/json;charset=UTF-8' },
          keepalive: true,
          mode: 'cors',
          credentials: 'omit'
        }).catch(function () {});
      }
    } catch (e) {
      /* never break the host page */
    }
  }

  /** The payload shared by every hit. */
  function basePayload() {
    var payload = {
      website: WEBSITE_ID || WEBSITE,
      url: currentUrl(),
      hostname: window.location.hostname,
      language: language,
      screen: screen,
      title: document.title
    };
    var ref = referrer();
    if (ref) payload.referrer = ref;
    return payload;
  }

  /** Track a pageview (automatic on load). */
  function trackPageview() {
    send('event', basePayload());
  }

  /**
   * Track a custom event.
   * @param {string} name
   * @param {object} [data]
   */
  function track(name, data) {
    if (!name) return;
    var payload = basePayload();
    payload.name = name;
    if (data) payload.data = data;
    send('event', payload);
  }

  /**
   * Attach data to the current session.
   * @param {object} data
   */
  function identify(data) {
    var payload = basePayload();
    payload.data = data || {};
    send('identify', payload);
  }

  // The public API. `umami` is an alias for compatibility with existing snippets.
  var api = { track: track, identify: identify, pageview: trackPageview };
  window.sumami = api;
  window.umami = api;

  if (document.readyState === 'complete') {
    trackPageview();
  } else {
    window.addEventListener('load', trackPageview);
  }
})();
