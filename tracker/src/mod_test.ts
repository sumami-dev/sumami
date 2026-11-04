// The tracker, tested without a browser.
//
// The package injects a script and hands the calls over to it. This test
// stands in for the browser: a fake document, and a fake tracker that appears
// when the "script" loads.
//
// The module keeps its state in globals — as a browser script has to — so the
// whole flow is one test. The "not started yet" case is in its own file, which
// gets its own copy of the module.

import { assertEquals } from "jsr:@std/assert@1";

import { init } from "./mod.ts";

/** What the tracker recorded. */
const calls: unknown[][] = [];

/** The script the package injected. */
let injected: {
  src: string;
  attributes: Record<string, string>;
} | undefined;

/** Install a fake document. */
function fakeDom(): void {
  const listeners: (() => void)[] = [];
  const document = {
    createElement: () => {
      const element = {
        async: false,
        src: "",
        attributes: {} as Record<string, string>,
        setAttribute(name: string, value: string) {
          element.attributes[name] = value;
        },
        addEventListener(_type: string, listener: () => void) {
          listeners.push(listener);
        },
      };
      injected = element;
      return element;
    },
    head: {
      appendChild() {
        // The script "loads" right away.
        for (const listener of listeners) listener();
      },
    },
  };
  (globalThis as unknown as { document: unknown }).document = document;
}

Deno.test("the tracker is injected, and the calls reach it", () => {
  fakeDom();

  // The tracker has not loaded yet: the call has to wait.
  const sumimi = init({ website: "w1", apiUrl: "https://analytics.example" });
  sumimi.track("signup", { plan: "pro" });

  assertEquals(injected?.src, "https://analytics.example/sumimi.js");
  assertEquals(injected?.attributes["data-website-id"], "w1");
  assertEquals(injected?.attributes["data-api-url"], "https://analytics.example");
  assertEquals(calls.length, 0, "nothing was sent yet");

  // Now the tracker arrives.
  (globalThis as unknown as { window: unknown }).window = {
    sumimi: {
      track: (name: string, data?: unknown) => calls.push(["track", name, data]),
      identify: (data: unknown) => calls.push(["identify", data]),
      pageview: (url?: string) => calls.push(["pageview", url]),
    },
  };
  // The script's "load" event fires.
  (globalThis as unknown as { window: unknown; document: { head: { appendChild: () => void } } })
    .document.head.appendChild();

  assertEquals(calls.length, 1, "the queued call went out");
  assertEquals(calls[0][0], "track");
  assertEquals(calls[0][1], "signup");
  assertEquals(calls[0][2], { plan: "pro" });

  // And a call made now goes straight out.
  sumimi.identify({ email: "a@example.com" });
  assertEquals(calls.length, 2);
  assertEquals(calls[1][0], "identify");

  (globalThis as unknown as { window?: unknown }).window = undefined;
  (globalThis as unknown as { document?: unknown }).document = undefined;
});
