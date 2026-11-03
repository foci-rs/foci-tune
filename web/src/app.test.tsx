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

test("top bar shows klipper state and kill; the sidebar holds nav and printer status", async () => {
  respond(printerView());
  await mount();
  const header = screen.getByRole("banner");
  expect(await within(header).findByText("Klipper ready")).toBeInTheDocument();
  expect(within(header).getByRole("button", { name: "Kill" })).toBeInTheDocument();
  expect(header).not.toHaveTextContent("Homed");

  const nav = screen.getByRole("navigation", { name: "Pages" });
  expect(within(nav).getByRole("link", { name: "Printer" })).toBeInTheDocument();
  expect(within(nav).getByRole("link", { name: "Tune" })).toBeInTheDocument();
  expect(within(nav).getByRole("link", { name: "Sessions" })).toBeInTheDocument();

  const status = screen.getByRole("region", { name: "Printer status" });
  expect(status).toHaveTextContent("Moonraker");
  expect(status).toHaveTextContent("Trace");
  expect(status).toHaveTextContent("Homed xy");
  expect(status).toHaveTextContent("Free 3 GB");
  expect(within(status).getByRole("button", { name: "Home" })).toBeInTheDocument();
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

test("home posts once, disables while homing and shows a refusal", async () => {
  let release: (r: Response) => void = () => {};
  respond(printerView());
  const base = fetchMock.getMockImplementation() as (p: string, i?: RequestInit) => Promise<Response>;
  fetchMock.mockImplementation(async (path: string, init?: RequestInit) =>
    path === "/api/home" ? new Promise<Response>((r) => (release = r)) : base(path, init),
  );
  await mount();
  const button = await screen.findByRole("button", { name: "Home" });
  await act(async () => {
    fireEvent.click(button);
  });
  expect(fetchMock.mock.calls.filter(([p]) => p === "/api/home")).toHaveLength(1);
  expect(screen.getByRole("button", { name: "Homing" })).toBeDisabled();
  await act(async () => {
    release(new Response(JSON.stringify({ error: "a run is in progress" }), { status: 409 }));
  });
  expect(await screen.findByText("a run is in progress")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Home" })).toBeEnabled();
});

test("home is offered only while klipper is ready", async () => {
  respond(printerView({ klippy: "disconnected" }));
  await mount();
  expect(await screen.findByRole("button", { name: "Home" })).toBeDisabled();
});

test("a home refusal clears once klipper's state changes", async () => {
  respond(printerView());
  const base = fetchMock.getMockImplementation() as (p: string, i?: RequestInit) => Promise<Response>;
  fetchMock.mockImplementation(async (path: string, init?: RequestInit) =>
    path === "/api/home"
      ? new Response(JSON.stringify({ error: "Klippy Host not connected" }), { status: 502 })
      : base(path, init),
  );
  await mount();
  await act(async () => {
    fireEvent.click(await screen.findByRole("button", { name: "Home" }));
  });
  expect(await screen.findByText("Klippy Host not connected")).toBeInTheDocument();

  await act(async () => {
    StubSocket.last!.deliver({ type: "printer", ...printerView({ klippy: "disconnected" }) });
  });

  expect(screen.queryByText("Klippy Host not connected")).not.toBeInTheDocument();
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
  act(() => StubSocket.last!.deliver({ type: "run_state", session: "s", run: "r", state: "preparing" }));
  expect(screen.queryByText("Stop confirmed")).toBeNull();
});
