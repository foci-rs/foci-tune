import { act, render } from "@testing-library/react";
import { StrictMode, useEffect } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { FeedProvider, useFeed } from "./feed";

class Sock {
  static all: Sock[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  sent: string[] = [];
  open = false;
  constructor() {
    Sock.all.push(this);
  }
  send(data: string) {
    if (!this.open) throw new Error("InvalidStateError");
    this.sent.push(data);
  }
  close() {
    this.open = false;
    this.onclose?.();
  }
}

function Consumer() {
  const { subscribeLive } = useFeed();
  useEffect(() => subscribeLive(), [subscribeLive]);
  return null;
}

beforeEach(() => {
  Sock.all = [];
  vi.stubGlobal("WebSocket", Sock);
  vi.stubGlobal("fetch", () => new Promise(() => {}));
});
afterEach(() => {
  vi.unstubAllGlobals();
});

test("under strict mode exactly one live subscription reaches the open socket", () => {
  const { unmount } = render(
    <StrictMode>
      <FeedProvider>
        <Consumer />
      </FeedProvider>
    </StrictMode>,
  );
  const socket = Sock.all[Sock.all.length - 1]!;
  act(() => {
    socket.open = true;
    socket.onopen?.();
  });
  expect(socket.sent).toEqual([JSON.stringify({ subscribe: "live" })]);
  unmount();
});

test("the last unsubscribe sends unsubscribe", () => {
  function Toggle({ on }: { on: boolean }) {
    return <FeedProvider>{on && <Consumer />}</FeedProvider>;
  }
  const { rerender } = render(<Toggle on />);
  const socket = Sock.all[0]!;
  act(() => {
    socket.open = true;
    socket.onopen?.();
  });
  expect(socket.sent).toEqual([JSON.stringify({ subscribe: "live" })]);
  rerender(<Toggle on={false} />);
  expect(socket.sent).toEqual([JSON.stringify({ subscribe: "live" }), JSON.stringify({ unsubscribe: "live" })]);
});
