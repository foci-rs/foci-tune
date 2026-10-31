import { RouterProvider } from "@tanstack/react-router";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { PrinterView } from "./api";
import { createTestRouter } from "./router";

class StubSocket {
  static last: StubSocket | null = null;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor() {
    StubSocket.last = this;
  }
  send() {}
  close() {}
  deliver(obj: unknown) {
    this.onmessage?.({ data: JSON.stringify(obj) });
  }
}

function printerView(over: Partial<PrinterView["moonraker"]> = {}, unconfirmed: string | null = null): PrinterView {
  return {
    moonraker: {
      connected: true,
      klippy: "ready",
      homed_axes: "xy",
      print_state: "standby",
      idle_state: "Idle",
      max_velocity: 500,
      max_accel: 10000,
      ...over,
    },
    steppers: [],
    settings_error: null,
    boards: [],
    preconditions: [],
    free_bytes: 3 * 1024 ** 3,
    engine: { current: null, queue: [], unconfirmed_kill: unconfirmed },
  };
}

let fetchMock: ReturnType<typeof vi.fn>;

function respond(view: PrinterView, kill?: () => Response | Promise<Response>) {
  fetchMock.mockImplementation(async (path: string, init?: RequestInit) => {
    if (path === "/api/printer") return new Response(JSON.stringify(view));
    if (path === "/api/kill" && init?.method === "POST" && kill) return kill();
    return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 });
  });
}

async function mount() {
  render(<RouterProvider router={createTestRouter("/printer")} />);
  await screen.findByRole("banner");
}

beforeEach(() => {
  fetchMock = vi.fn();
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("WebSocket", StubSocket);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

test("header shows klipper state, homed axes, free space and nav", async () => {
  respond(printerView());
  await mount();
  const header = screen.getByRole("banner");
  expect(await within(header).findByText("Klipper ready")).toBeInTheDocument();
  expect(header).toHaveTextContent("Homed xy");
  expect(header).toHaveTextContent("Free 3 GB");
  expect(within(header).getByRole("link", { name: "Tune" })).toBeInTheDocument();
  expect(within(header).getByRole("link", { name: "Sessions" })).toBeInTheDocument();
});

test("kill needs a second click and posts once", async () => {
  respond(printerView(), () => new Response(JSON.stringify({ run: null, state: "killed" })));
  await mount();
  const button = await screen.findByRole("button", { name: "Kill" });
  fireEvent.click(button);
  expect(screen.getByRole("button", { name: "Press again to kill" })).toBeInTheDocument();
  expect(fetchMock.mock.calls.filter(([p]) => p === "/api/kill")).toHaveLength(0);
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Press again to kill" }));
  });
  expect(fetchMock.mock.calls.filter(([p]) => p === "/api/kill")).toHaveLength(1);
  expect(await screen.findByText("Stop confirmed")).toBeInTheDocument();
});

test("an unconfirmed kill shows the banner and re-posts on one click", async () => {
  respond(printerView(), () => new Response(JSON.stringify({ run: "r", state: "kill_unconfirmed" })));
  await mount();
  fireEvent.click(await screen.findByRole("button", { name: "Kill" }));
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Press again to kill" }));
  });
  expect(await screen.findByRole("alert")).toHaveTextContent("motion may continue");
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Kill again" }));
  });
  expect(fetchMock.mock.calls.filter(([p]) => p === "/api/kill")).toHaveLength(2);
});

test("firmware restart shows only when klipper is shut down", async () => {
  respond(printerView());
  await mount();
  await screen.findByText("Klipper ready");
  expect(screen.queryByRole("button", { name: "Firmware restart" })).toBeNull();
});

test("firmware restart appears on shutdown", async () => {
  respond(printerView({ klippy: "shutdown" }));
  await mount();
  expect(await screen.findByRole("button", { name: "Firmware restart" })).toBeInTheDocument();
});

async function killOnce() {
  respond(printerView(), () => new Response(JSON.stringify({ run: null, state: "killed" })));
  await mount();
  fireEvent.click(await screen.findByRole("button", { name: "Kill" }));
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Press again to kill" }));
  });
  await screen.findByText("Stop confirmed");
}

test("the confirmed stop clears when klipper returns to ready", async () => {
  await killOnce();
  const socket = StubSocket.last!;
  act(() => socket.deliver({ type: "printer", ...printerView({ klippy: "shutdown" }) }));
  await screen.findByText("Klipper shutdown");
  expect(screen.getByText("Stop confirmed")).toBeInTheDocument();
  act(() => socket.deliver({ type: "printer", ...printerView() }));
  await screen.findByText("Klipper ready");
  expect(screen.queryByText("Stop confirmed")).toBeNull();
});

test("the confirmed stop clears when a new run starts", async () => {
  await killOnce();
  act(() =>
    StubSocket.last!.deliver({ type: "run_state", session: "s", run: "r", state: "preparing" }),
  );
  expect(screen.queryByText("Stop confirmed")).toBeNull();
});
