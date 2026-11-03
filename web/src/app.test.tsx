import { RouterProvider } from "@tanstack/react-router";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { BoardView, PrinterView, StepperView } from "./api";
import { connectionDetails, connectionStatuses, homedTone } from "./components/header";
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
    version: "0.1.0",
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
  expect(within(header).getByRole("link", { name: /FOCI/ })).toHaveAttribute("href", "/printer");

  const nav = screen.getByRole("navigation", { name: "Pages" });
  expect(within(nav).queryByRole("link", { name: /FOCI/ })).toBeNull();
  expect(within(nav).getByRole("link", { name: "Printer" })).toBeInTheDocument();
  expect(within(nav).getByRole("link", { name: "Tune" })).toBeInTheDocument();
  expect(within(nav).getByRole("link", { name: "Sessions" })).toBeInTheDocument();

  const status = screen.getByRole("region", { name: "Printer status" });
  expect(status).not.toHaveTextContent("Moonraker");
  expect(status).not.toHaveTextContent("Homed");
  expect(status).toHaveTextContent("Disk Free 3 GB");
  expect(status).toHaveTextContent("Version 0.1.0");
  expect(within(status).queryByRole("button", { name: "Home" })).toBeNull();
  const killButton = within(header).getByRole("button", { name: "Kill" });
  const homeButton = within(header).getByRole("button", { name: "Home" });
  expect(killButton.compareDocumentPosition(homeButton) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test("the kill button stays an icon: a tooltip explains the double click and the first click inverts it", async () => {
  respond(printerView());
  await mount();
  const button = await screen.findByRole("button", { name: "Kill" });
  expect(button).toHaveAttribute("aria-pressed", "false");
  fireEvent.focus(button);
  const tip = await screen.findByRole("tooltip");
  expect(tip).toHaveTextContent("Emergency stop");
  expect(tip).toHaveTextContent("twice");

  fireEvent.click(button);

  const armed = screen.getByRole("button", { name: "Press again to kill" });
  expect(armed).toHaveAttribute("aria-pressed", "true");
  expect(armed).toHaveTextContent("");
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

test("homed tone is full only with x, y and z, partial with any axis", () => {
  expect(homedTone("xyz")).toBe("full");
  expect(homedTone("zyx")).toBe("full");
  expect(homedTone("xy")).toBe("partial");
  expect(homedTone("")).toBe("none");
});

test("the home button names the homed axes in its tooltip", async () => {
  respond(printerView({ homed_axes: "xy" }));
  await mount();
  expect(await screen.findByRole("button", { name: "Home" })).toHaveAttribute("title", "Home all axes (homed: xy)");
});

test("the home button says nothing is homed", async () => {
  respond(printerView({ homed_axes: "" }));
  await mount();
  expect(await screen.findByRole("button", { name: "Home" })).toHaveAttribute("title", "Home all axes (homed: none)");
});

const board = (over: Partial<BoardView> = {}): BoardView =>
  ({
    serial: "b",
    open: true,
    trace_mode: "streaming",
    channels: [],
    critical_hz_measured: 50000,
    ...over,
  }) as BoardView;

test("the top bar lists moonraker, klipper, foci and tracing in that order", async () => {
  respond(printerView());
  await mount();
  const items = within(screen.getByRole("list", { name: "Connections" })).getAllByRole("listitem");
  expect(items.map((li) => li.querySelector("[aria-hidden=true]:not(:empty)")?.textContent)).toEqual([
    "Moonraker",
    "Klipper",
    "FOCI",
    "Tracing",
  ]);
});

test("connection statuses follow moonraker, klippy, board and trace state", () => {
  const base = printerView();
  expect(connectionStatuses(null)).toMatchObject({
    moonraker: "off",
    klipper: "off",
    foci: "off",
    tracing: "off",
  });
  expect(connectionStatuses(base)).toMatchObject({ moonraker: "ok", klipper: "ok", foci: "off" });
  expect(connectionStatuses(printerView({ klippy: "startup" })).klipper).toBe("warn");
  expect(connectionStatuses(printerView({ klippy: "shutdown" })).klipper).toBe("error");
  const withBoards = (boards: BoardView[]) => ({ ...base, boards });
  expect(connectionStatuses(withBoards([board(), board()]))).toMatchObject({
    foci: "ok",
    tracing: "ok",
  });
  expect(connectionStatuses(withBoards([board(), board({ open: false })])).foci).toBe("warn");
  expect(connectionStatuses(withBoards([board({ trace_mode: "dormant" })])).tracing).toBe("off");
  expect(connectionStatuses(withBoards([board(), board({ trace_mode: "dormant" })])).tracing).toBe("warn");
});

test("connection details list each board's state and why a board is unusable", () => {
  const stepper = (mcu: string, serial: string) => ({ mcu, serial }) as StepperView;
  const view = {
    ...printerView(),
    steppers: [stepper("ouroboros", "AAA"), stepper("tool_head", "AAA")],
    boards: [
      board({ serial: "AAA", product: "Ouroboros", firmware_id: "fw-1", critical_hz_measured: 49987 }),
      board({
        serial: "BBB",
        open: false,
        unusable_reason: "protocol mismatch",
        trace_mode: "dormant",
        undecodable_frames: 3,
      }),
    ],
  };
  const d = connectionDetails(view);
  expect(d.foci[0]).toBe("Ouroboros: open");
  expect(d.tracing[0]).toBe("Ouroboros: Streaming");
  expect(d.foci.join("\n")).not.toContain("AAA");
  expect(d.foci[1]).toBe("BBB: closed");
  expect(d.tracing[1]).toBe("BBB: Off");
  expect(connectionDetails(printerView()).foci).toEqual(["No boards found"]);
  expect(connectionDetails(null).moonraker).toEqual(["Not connected to Moonraker"]);
});

test("focusing a status item shows its details", async () => {
  respond({ ...printerView(), boards: [board({ serial: "AAA" })] });
  await mount();
  const label = await screen.findByText("FOCI", { selector: "[aria-hidden=true]" });
  fireEvent.focus(label.closest("[data-slot=tooltip-trigger]")!);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("AAA");
});
