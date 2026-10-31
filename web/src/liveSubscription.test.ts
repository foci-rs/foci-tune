import { expect, test } from "vitest";
import { createLiveSubscriptions } from "./liveSubscription";

function fakeClient() {
  const calls: string[] = [];
  return {
    calls,
    client: {
      subscribe: () => calls.push("subscribe"),
      unsubscribe: () => calls.push("unsubscribe"),
      close: () => calls.push("close"),
    },
  };
}

test("a subscription taken before the client exists is applied when it attaches", () => {
  const live = createLiveSubscriptions();
  live.acquire();
  const { client, calls } = fakeClient();
  live.attach(client);
  expect(calls).toEqual(["subscribe"]);
});

test("a recreated client gets the live subscription again", () => {
  const live = createLiveSubscriptions();
  live.acquire();
  const a = fakeClient();
  live.attach(a.client);
  live.attach(null);
  const b = fakeClient();
  live.attach(b.client);
  expect(b.calls).toEqual(["subscribe"]);
});

test("a strict-mode mount, unmount and remount leaves one subscription", () => {
  const live = createLiveSubscriptions();
  const first = live.acquire();
  first();
  const second = live.acquire();
  const { client, calls } = fakeClient();
  live.attach(client);
  expect(calls).toEqual(["subscribe"]);
  second();
  expect(calls).toEqual(["subscribe", "unsubscribe"]);
});

test("only the first user subscribes and only the last unsubscribes", () => {
  const live = createLiveSubscriptions();
  const { client, calls } = fakeClient();
  live.attach(client);
  const a = live.acquire();
  const b = live.acquire();
  expect(calls).toEqual(["subscribe"]);
  a();
  a();
  expect(calls).toEqual(["subscribe"]);
  b();
  expect(calls).toEqual(["subscribe", "unsubscribe"]);
});
