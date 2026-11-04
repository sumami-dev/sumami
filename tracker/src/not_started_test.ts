// The tracker has to be started before it can be used.
//
// This file gets its own copy of the module — so the state is clean here.

import { assertEquals } from "jsr:@std/assert@1";

import { track } from "./mod.ts";

Deno.test("track says what to do when the tracker is not running", () => {
  let message = "";
  try {
    track("nope");
  } catch (error) {
    message = error instanceof Error ? error.message : String(error);
  }
  assertEquals(
    message.includes("init"),
    true,
    "the error says how to start it: " + message,
  );
});
