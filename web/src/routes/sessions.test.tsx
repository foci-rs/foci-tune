import { RouterProvider } from "@tanstack/react-router";
import { render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { SessionView } from "../api";
import { createTestRouter } from "../router";

class StubSocket {
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  send() {}
  close() {}
}

const session = (id: string, over: Partial<SessionView> = {}): SessionView => ({
  id,
  error: null,
  session: {
    format_version: 1,
    name: id,
    created_unix_s: 1_700_000_000,
    stepper: "stepper_x",
    kind: { Probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 } },
    pattern: { start: [100, 50], end: [200, 50] },
  } as SessionView["session"],
  runs: [],
  ...over,
});

const broken = session("20261030-bbb", {
  runs: [
    { id: "r1", state: "failed", reason: "run.json: truncated", run: null },
    { id: "r2", state: "skipped", reason: "ladder floor reached", run: null },
  ],
});
const sessions = [session("20261029-aaa"), broken];

beforeEach(() => {
  vi.stubGlobal("WebSocket", StubSocket);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      if (path === "/api/sessions") return new Response(JSON.stringify(sessions));
      if (path === "/api/sessions/20261030-bbb") return new Response(JSON.stringify(broken));
      return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 });
    }),
  );
});
afterEach(() => vi.unstubAllGlobals());

test("the list shows newest first with each state count and reason", async () => {
  render(<RouterProvider router={createTestRouter("/sessions")} />);
  const table = await screen.findByRole("table");
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows[0]).toHaveTextContent("20261030-bbb");
  expect(rows[1]).toHaveTextContent("20261029-aaa");
  expect(rows[0]).toHaveTextContent("1 failed");
  expect(rows[0]).toHaveTextContent("1 skipped");
  expect(rows[0]).toHaveTextContent("run.json: truncated");
  expect(rows[0]).toHaveTextContent("n/a");
});

test("an opened session shows its broken runs' reasons and no control that acts on the printer", async () => {
  render(<RouterProvider router={createTestRouter("/sessions/20261030-bbb")} />);
  expect((await screen.findAllByText(/incomplete run directory/)).length).toBe(2);
  expect(screen.getAllByText("run.json: truncated").length).toBeGreaterThan(0);
  expect(screen.getByText("ladder floor reached")).toBeInTheDocument();
  const names = screen.getAllByRole("button").map((b) => b.textContent);
  expect(names).toContain("Accept");
  for (const forbidden of [/run/i, /revert/i, /delete/i, /remove/i]) {
    expect(screen.queryByRole("button", { name: forbidden })).toBeNull();
  }
});
