import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { ApiRequestError, postKill } from "./api";

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
        init?.signal?.addEventListener("abort", () =>
          reject(new DOMException("aborted", "AbortError")),
        );
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
