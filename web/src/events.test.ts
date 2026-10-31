import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import {
  connectEvents,
  parseServerMessage,
  type ServerMessage,
  type SocketLike,
} from "./api";

class FakeSocket implements SocketLike {
  static all: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  sent: string[] = [];
  closed = false;
  constructor(readonly url: string) {
    FakeSocket.all.push(this);
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.closed = true;
    this.onclose?.();
  }
  open() {
    this.onopen?.();
  }
  deliver(obj: unknown) {
    this.onmessage?.({ data: typeof obj === "string" ? obj : JSON.stringify(obj) });
  }
}

beforeEach(() => {
  FakeSocket.all = [];
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

function start() {
  const messages: ServerMessage[] = [];
  const status: boolean[] = [];
  const resyncs: string[] = [];
  const client = connectEvents({
    url: "ws://x/api/events",
    onMessage: (m) => messages.push(m),
    onStatus: (c) => status.push(c),
    onResync: (r) => resyncs.push(r),
    createSocket: (url) => new FakeSocket(url),
    baseDelayMs: 500,
    maxDelayMs: 2000,
  });
  return { client, messages, status, resyncs };
}

describe("event client", () => {
  test("dispatches typed messages and reports resync", () => {
    const { messages, resyncs, status } = start();
    const s = FakeSocket.all[0]!;
    s.open();
    s.deliver({ type: "queue", runs: ["a", "b"] });
    s.deliver({ type: "resync", reason: "lagged" });
    expect(status).toEqual([true]);
    expect(messages.map((m) => m.type)).toEqual(["queue", "resync"]);
    expect(resyncs).toEqual(["lagged"]);
  });

  test("ignores malformed and unknown messages", () => {
    const { messages } = start();
    const s = FakeSocket.all[0]!;
    s.open();
    s.deliver("not json");
    s.deliver({ type: "mystery" });
    s.deliver({ nothing: 1 });
    expect(messages).toEqual([]);
  });

  test("reconnects with doubling backoff capped, reset by a successful open", () => {
    const { status } = start();
    FakeSocket.all[0]!.onclose?.();
    expect(FakeSocket.all).toHaveLength(1);
    vi.advanceTimersByTime(499);
    expect(FakeSocket.all).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.all).toHaveLength(2);
    FakeSocket.all[1]!.onclose?.();
    vi.advanceTimersByTime(999);
    expect(FakeSocket.all).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.all).toHaveLength(3);
    FakeSocket.all[2]!.onclose?.();
    vi.advanceTimersByTime(2000);
    expect(FakeSocket.all).toHaveLength(4);
    FakeSocket.all[3]!.open();
    FakeSocket.all[3]!.onclose?.();
    vi.advanceTimersByTime(500);
    expect(FakeSocket.all).toHaveLength(5);
    expect(status).toEqual([false, false, false, true, false]);
  });

  test("replays the live subscription after a reconnect", () => {
    const { client } = start();
    FakeSocket.all[0]!.open();
    client.subscribe("live");
    expect(FakeSocket.all[0]!.sent).toEqual([JSON.stringify({ subscribe: "live" })]);
    FakeSocket.all[0]!.onclose?.();
    vi.advanceTimersByTime(500);
    FakeSocket.all[1]!.open();
    expect(FakeSocket.all[1]!.sent).toEqual([JSON.stringify({ subscribe: "live" })]);
  });

  test("close stops reconnecting", () => {
    const { client } = start();
    client.close();
    vi.advanceTimersByTime(10_000);
    expect(FakeSocket.all).toHaveLength(1);
  });

  test("parseServerMessage keeps the run state reason", () => {
    const m = parseServerMessage(
      JSON.stringify({
        type: "run_state",
        session: "s",
        run: "r",
        state: "failed",
        reason: "boom",
      }),
    );
    expect(m).toMatchObject({ type: "run_state", state: "failed", reason: "boom" });
  });
});
