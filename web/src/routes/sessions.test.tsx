import { RouterProvider } from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
const tune = (id: string, state: string, created: number) => ({
  id,
  stepper: "stepper_y",
  state,
  created,
  current_step: null,
});
const tunes = [tune("t-open", "Running", 5), tune("t-done", "Done", 3), tune("t-gone", "Abandoned", 4)];

beforeEach(() => {
  vi.stubGlobal("WebSocket", StubSocket);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      if (path === "/api/sessions") return new Response(JSON.stringify(sessions));
      if (path === "/api/tunes") return new Response(JSON.stringify(tunes));
      if (path === "/api/sessions/20261030-bbb") return new Response(JSON.stringify(broken));
      return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 });
    }),
  );
});
afterEach(() => vi.unstubAllGlobals());

test("the list has one row per session, newest first, with start, stepper, test and run count", async () => {
  render(<RouterProvider router={createTestRouter("/sessions")} />);
  const table = await screen.findByRole("table");
  const headers = within(table)
    .getAllByRole("columnheader")
    .map((h) => h.textContent);
  expect(headers).toEqual(["Started", "Stepper", "Test", "Runs"]);
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows).toHaveLength(2);
  expect(rows[0]).toHaveTextContent(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}stepper_xProbe2$/);
  expect(rows[0]).not.toHaveTextContent(/failed|skipped|n\/a|truncated/);
  expect(within(rows[0]).getByRole("link")).toHaveAttribute("href", "/sessions/20261030-bbb");
  expect(within(rows[1]).getByRole("link")).toHaveAttribute("href", "/sessions/20261029-aaa");
});

test("an opened session shows its broken runs' reasons and no control that acts on the printer", async () => {
  render(<RouterProvider router={createTestRouter("/sessions/20261030-bbb")} />);
  expect((await screen.findAllByText(/incomplete run directory/)).length).toBe(2);
  expect(screen.getAllByText(/run\.json: truncated/).length).toBeGreaterThan(0);
  expect(screen.getByText("ladder floor reached")).toBeInTheDocument();
  for (const forbidden of [/run/i, /revert/i, /delete/i, /remove/i]) {
    expect(screen.queryByRole("button", { name: forbidden })).toBeNull();
  }
});

const fullRun = (id: string, state: string, createdAt: number, value: number | null = null, velocityI = 412) => ({
  id,
  state,
  reason: state === "skipped" ? "ladder stopped: damping unassessable at velocity_i_corner_hz 25" : null,
  run: {
    id,
    request: {
      ladder_value: value,
      gains: { velocity_p: 841, velocity_i: velocityI, position_p: 120 },
      filters: { velocity_hz: 200, torque_hz: 1000, position_hz: 300, flux_hz: 1000 },
    },
    state,
    verification:
      state === "done"
        ? [
            { check: "channel_moved", passed: true, detail: "" },
            { check: "no_drop_gaps", passed: true, detail: "" },
          ]
        : [],
    metrics:
      state === "done"
        ? {
            channels: [
              {
                channel: 0,
                declared: true,
                tracking: [],
                standstill: null,
                probe: {
                  velocity: { status: "fit", zeta: 0.039, damped_hz: 151.6 },
                  edges: 134,
                  torque_psd_peak_hz: 152.3,
                },
              },
            ],
          }
        : null,
    baselines: [],
    steps: [{ step: "queued", unix_ms: createdAt }],
    notes: [],
    clamped_speeds: [],
    applied_speeds: [],
    declared_channel: 0,
    created_unix_s: 1,
  },
});

const reviewed = session("20261104-review", {
  runs: [
    fullRun("a1", "done", 1_000),
    fullRun("a2", "done", 2_000),
    fullRun("a3", "skipped", 3_000),
  ] as unknown as SessionView["runs"],
});

function openReviewed() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      if (path === "/api/sessions/20261104-review") return new Response(JSON.stringify(reviewed));
      return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 });
    }),
  );
  render(<RouterProvider router={createTestRouter("/sessions/20261104-review")} />);
}

test("a skipped run has no data to overlay, so its checkbox is disabled", async () => {
  openReviewed();
  const boxes = await screen.findAllByRole("checkbox", { name: /Overlay run/ });
  expect(boxes).toHaveLength(3);
  expect(boxes.filter((b) => (b as HTMLInputElement).disabled)).toHaveLength(1);
});

test("a run's checkbox adds it to the overlay and takes it off again", async () => {
  openReviewed();
  const [first] = await screen.findAllByRole("checkbox", { name: /Overlay run/ });
  expect(first).toHaveAttribute("aria-checked", "false");
  fireEvent.click(first);
  await waitFor(() => expect(first).toHaveAttribute("aria-checked", "true"));
  fireEvent.click(first);
  await waitFor(() => expect(first).toHaveAttribute("aria-checked", "false"));
});

/** The results table, not the plot's data table. */
async function resultsTable() {
  const tables = await screen.findAllByRole("table");
  const table = tables.find((t) => within(t).queryByRole("columnheader", { name: "Outcome" }) !== null);
  if (table === undefined) throw new Error("no results table");
  return table;
}

const sweepSession = (runs: unknown[]) =>
  session("20261104-order", {
    session: {
      ...session("20261104-order").session,
      ladder: { axis: "velocity_i_corner_hz", values: [25, 50], repeats: 2, floor: 0.05, marginal: 0.08 },
    } as SessionView["session"],
    runs: runs as SessionView["runs"],
  });

const fetchOf = (view: SessionView) => async (path: string) => {
  if (path === `/api/sessions/${view.id}`) return new Response(JSON.stringify(view));
  return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 });
};

function openSession(view: SessionView) {
  vi.stubGlobal("fetch", vi.fn(fetchOf(view)));
  render(<RouterProvider router={createTestRouter(`/sessions/${view.id}`)} />);
}

test("the results table lists a sweep by value, with repeats together, not by finish time", async () => {
  openSession(
    sweepSession([
      fullRun("r1", "done", 1_000, 25, 206),
      fullRun("r2", "done", 2_000, 50, 412),
      fullRun("r3", "done", 3_000, 25, 206),
      fullRun("r4", "done", 4_000, 50, 412),
    ]),
  );
  const table = await resultsTable();
  const rows = within(table).getAllByRole("row").slice(1);
  const headers = within(table)
    .getAllByRole("columnheader")
    .map((h) => h.textContent);
  const at = headers.indexOf("Velocity I (Hz)");
  const ladderValue = (r: HTMLElement) => parseFloat(within(r).getAllByRole("cell")[at].textContent ?? "");
  expect(rows.map(ladderValue)).toEqual([25, 25, 50, 50]);
});

test("no cell is marked as changed, even where a run's gains differ from the one before", async () => {
  openSession(
    sweepSession([
      fullRun("r1", "done", 1_000, 25, 206),
      fullRun("r2", "done", 2_000, 50, 412),
      fullRun("r3", "skipped", 3_000, 75, 618),
    ]),
  );
  const table = await resultsTable();
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows).toHaveLength(3);
  for (const row of rows) expect(row).not.toHaveTextContent("changed");
  expect(table.querySelector(".bg-emerald-900\\/50")).toBeNull();
});

const sweepRuns = () => [
  fullRun("r1", "done", 1_000, 25, 206),
  fullRun("r2", "done", 2_000, 50, 412),
  {
    ...fullRun("r3", "not_comparable", 3_000, 75, 618),
    run: {
      ...fullRun("r3", "not_comparable", 3_000, 75, 618).run,
      verification: [{ check: "channel_moved", passed: false, detail: "channel 1 moved" }],
    },
  },
  fullRun("r4", "skipped", 4_000, 100, 824),
];

test("the table is short and plain: settings that never change sit in two lines under the title", async () => {
  openSession(sweepSession(sweepRuns()));
  const table = await resultsTable();
  const card = screen.getByRole("region", { name: /^Results/ });
  expect(card).not.toHaveTextContent("Gains:");
  expect(card).toHaveTextContent("Filters: Torque 1000 Hz · Flux 1000 Hz · Velocity 200 Hz · Position 300 Hz");
  const headers = within(table)
    .getAllByRole("columnheader")
    .map((h) => h.textContent);
  expect(headers).toEqual([
    "Overlay",
    "Time",
    "Velocity P",
    "Velocity I (Hz)",
    "Position P",
    "Damping",
    "Outcome",
    "Details",
  ]);
  const first = within(table).getAllByRole("row")[1];
  expect(first).toHaveTextContent("841");
  expect(first).toHaveTextContent("120");
});

test("a probe's damping comes with a bar coloured by the stop and marginal limits", async () => {
  openSession(sweepSession(sweepRuns()));
  const table = await resultsTable();
  const bar = within(within(table).getAllByRole("row")[1]).getByRole("meter", { name: "Damping 0.039" });
  expect(bar).toHaveAttribute("data-tone", "low");
  expect(bar).toHaveAttribute("aria-valuenow", "0.039");
});

test("outcomes read in plain words with one sentence of reason", async () => {
  openSession(sweepSession(sweepRuns()));
  const table = await resultsTable();
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows[0]).toHaveTextContent("Done");
  expect(rows[0]).toHaveTextContent("0.039");
  expect(rows[2]).toHaveTextContent("Rejected");
  expect(rows[2]).toHaveTextContent(
    "Another motor moved while stepper_x was being tested, so the result can't be trusted.",
  );
  expect(rows[3]).toHaveTextContent("Did not run");
  expect(rows[3]).toHaveTextContent("The sweep stopped after 25 Hz because its damping could not be measured.");
});

test("an expand button opens a panel with everything about that run, in plain words", async () => {
  openSession(sweepSession(sweepRuns()));
  fireEvent.click(await screen.findByRole("button", { name: "Details for 25 Hz" }));
  const panel = await screen.findByRole("dialog");
  expect(within(panel).getByRole("heading", { name: "Run at 25 Hz" })).toBeInTheDocument();
  for (const section of ["What was set", "What was measured", "Checks"]) {
    expect(within(panel).getByText(section)).toBeInTheDocument();
  }
  expect(panel).toHaveTextContent("Velocity P841");
  expect(panel).toHaveTextContent("Damping: 0.039");
  expect(panel).toHaveTextContent("Ringing frequency: 151.6 Hz");
  expect(panel).toHaveTextContent("Step responses recorded: 134");
});

test("a finished run can be accepted from its panel, which shows the config block", async () => {
  const base = fetchOf(sweepSession(sweepRuns()));
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      if (path === "/api/runs/r1/config-block") return new Response("[foci stepper_x]\npid_velocity_p: 841\n");
      return base(path);
    }),
  );
  render(<RouterProvider router={createTestRouter("/sessions/20261104-order")} />);
  fireEvent.click(await screen.findByRole("button", { name: "Details for 25 Hz" }));
  const panel = await screen.findByRole("dialog");
  fireEvent.click(within(panel).getByRole("button", { name: "Use these settings" }));
  expect(await within(panel).findByLabelText("Config block")).toHaveTextContent("pid_velocity_p: 841");
  expect(screen.queryByRole("button", { name: "Accept" })).toBeNull();
});

test("a run that did not run has nothing to accept", async () => {
  openSession(sweepSession(sweepRuns()));
  fireEvent.click(await screen.findByRole("button", { name: "Details for 100 Hz" }));
  const panel = await screen.findByRole("dialog");
  expect(within(panel).queryByRole("button", { name: "Use these settings" })).toBeNull();
});

test("finished and abandoned tunes are listed with a link to open them in the guided tune", async () => {
  render(<RouterProvider router={createTestRouter("/sessions")} />);
  const links = await screen.findAllByRole("link", { name: /stepper_y/ });
  expect(links.map((l) => l.getAttribute("href"))).toEqual(["/tune/guided?tune=t-gone", "/tune/guided?tune=t-done"]);
  expect(links[0]).toHaveTextContent("Abandoned");
  expect(links[1]).toHaveTextContent("Done");
});
