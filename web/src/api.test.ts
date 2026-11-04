import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { ApiRequestError, parseServerMessage, postKill, postTuneChoice, postTuneExtension, postTuneStep } from "./api";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

test("postKill gives up on a hung server so the kill reads as unconfirmed", async () => {
  vi.stubGlobal(
    "fetch",
    (_path: string, init?: RequestInit) =>
      new Promise((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
      }),
  );
  const result = postKill().then(
    () => null,
    (e: unknown) => e,
  );
  await vi.advanceTimersByTimeAsync(3100);
  const error = await result;
  expect(error).toBeInstanceOf(ApiRequestError);
  expect((error as ApiRequestError).status).toBe(0);
});

test("tune requests send the bodies the server decodes", async () => {
  const calls: { path: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", (path: string, init?: RequestInit) => {
    calls.push({ path, body: init?.body === undefined ? undefined : JSON.parse(String(init.body)) });
    return Promise.resolve(new Response("{}", { status: 200 }));
  });
  await postTuneStep("t 1", { seed: { typed: 1500 }, repeats: 2, lower_k: -3, upper_k: 3 });
  await postTuneChoice("t 1", { rung: 2 });
  await postTuneChoice("t 1", "recommended");
  await postTuneExtension("t 1", "below");
  expect(calls).toEqual([
    { path: "/api/tunes/t%201/steps", body: { seed: { typed: 1500 }, repeats: 2, lower_k: -3, upper_k: 3 } },
    { path: "/api/tunes/t%201/choice", body: { choice: { rung: 2 } } },
    { path: "/api/tunes/t%201/choice", body: { choice: "recommended" } },
    { path: "/api/tunes/t%201/extension", body: { direction: "below" } },
  ]);
});

test("the event parser accepts a tune message", () => {
  const message = parseServerMessage('{"type":"tune","tune":{"id":"t1"}}');
  expect(message?.type).toBe("tune");
});
