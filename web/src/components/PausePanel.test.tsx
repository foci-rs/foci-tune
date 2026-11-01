import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import type { FitOutcome, Pause, ProbeRecord, RepeatResult, Run, Series, Tune } from "@/api";
import { PausePanel } from "./PausePanel";

const getRun = vi.fn();
const getSeries = vi.fn();
vi.mock("@/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api")>()),
  getRun: (id: string) => getRun(id),
  getSeries: (id: string) => getSeries(id),
}));
vi.mock("./EnsemblePlot", () => ({
  EnsemblePlot: ({ runs }: { runs: { label: string; color: string }[] }) => (
    <ul aria-label="ensembles">
      {runs.map((r) => (
        <li key={r.label} data-color={r.color}>
          {r.label}
        </li>
      ))}
    </ul>
  ),
}));

const gains = { velocity_p: 1000, velocity_i: 50, position_p: 800, position_i: 0 };
const filters = { velocity_hz: 500, torque_hz: 1000, position_hz: 500, flux_hz: 1000 };

const fit = (zeta: number, starts: number): FitOutcome => ({
  status: "fit",
  damped_hz: 90,
  zeta,
  amplitude: 1,
  phase: 0,
  offset: 0,
  tau_s: 0.01,
  residual_rms: 0.1,
  admissible_starts: starts,
});

const probeRecord = (velocity: FitOutcome, edges: number): ProbeRecord => ({
  velocity,
  torque: fit(0.5, 3),
  position_error: fit(0.4, 3),
  edges,
  stderr_at_peak: 0.0125,
  torque_psd_peak_hz: null,
});

const result = (sd: number): RepeatResult => ({ zeta: "Unassessable", p99_by_speed: [3.5, 4.5], worst_torque_sd: sd });

function runWith(id: string, metrics: Run["metrics"]): Run {
  return { id, declared_channel: 0, metrics, state: "done" } as unknown as Run;
}

function pausedTune(reason: Pause = "DampingUnassessable"): Tune {
  const rung = (raw: number, ids: string[]) => ({
    shown: raw,
    raw,
    run_ids: ids,
    started_unix_ms: null,
    ended_unix_ms: null,
    skipped: null,
    repeat_values: [result(7.5)],
    run_times: [],
    waived: null,
    superseded: [],
  });
  return {
    format_version: 1,
    id: "t1",
    setup: {
      stepper: "stepper_x",
      speeds_mm_s: [50, 100],
      standstill: { rests: 3, dwell_s: 2 },
      gates: { damping_floor: 0.05, marginal: 0.08, standstill_factor: 2 },
    },
    snapshot: [gains, filters],
    steps: [
      {
        axis: "VelocityP",
        seed_raw: 1000,
        repeats: 1,
        lower_k: 0,
        upper_k: 1,
        rungs: [rung(1000, ["p0", "t0a", "t0b", "s0"]), rung(1260, ["p1", "t1a", "t1b", "s1"])],
        notes: [],
        paused: { rung: 1, reason, unix_ms: 1 },
        assessment: null,
        recommendation: null,
        drift: [],
        failed: null,
        standstill_gate_off: null,
      },
    ],
    state: "Paused",
    notes: [],
  } as unknown as Tune;
}

const plot = { t_s: [0], mean: [0], fit: null };
const series = {
  channels: [{ channel: 0, probe: { t_s: [0], velocity: plot, torque: plot, position_error: plot } }],
} as unknown as Series;

const standstill = (sds: number[]) => ({
  rests: sds.map((torque_sd, index) => ({ index, samples: 100, torque_sd, position_pp_um: 1 })),
  worst_torque_sd: Math.max(...sds),
});

beforeEach(() => {
  getRun.mockReset();
  getSeries.mockReset();
  getSeries.mockResolvedValue(series);
  getRun.mockImplementation(async (id: string) => {
    const channel = (probe: ProbeRecord | null, st: ReturnType<typeof standstill> | null) =>
      runWith(id, { channels: [{ channel: 0, declared: true, tracking: [], probe, standstill: st }] });
    if (id === "p1")
      return channel(probeRecord({ status: "failed", reason: "fitted damping ratio 1.70 is outside (0, 1)" }, 12), null);
    if (id === "p0") return channel(probeRecord(fit(0.09, 24), 41), null);
    if (id === "s1") return channel(null, standstill([2.5, 3.5]));
    return channel(null, null);
  });
});

test("offers the three decisions and posts the chosen one", async () => {
  const onDecide = vi.fn();
  render(<PausePanel tune={pausedTune()} step={0} busy={false} onDecide={onDecide} />);
  fireEvent.click(await screen.findByRole("button", { name: "Re-run this rung" }));
  expect(onDecide).toHaveBeenCalledWith("rerun");
  expect(screen.getByRole("button", { name: "Continue past this rung" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Stop the ladder" })).toBeEnabled();
});

test("lists each repeat's fit outcome, edge count and start count beside the previous rung", async () => {
  render(<PausePanel tune={pausedTune()} step={0} busy={false} onDecide={() => undefined} />);
  expect(await screen.findByText("fitted damping ratio 1.70 is outside (0, 1)")).toBeInTheDocument();
  expect(screen.getByText("41")).toBeInTheDocument();
  expect(screen.getByText("24")).toBeInTheDocument();
  expect(screen.getAllByText(/^0\.09\d$/).length).toBeGreaterThan(0);
});

test("plots the paused rung's repeats and dims the previous rung's", async () => {
  render(<PausePanel tune={pausedTune()} step={0} busy={false} onDecide={() => undefined} />);
  const items = await screen.findAllByRole("listitem");
  expect(items).toHaveLength(2);
  expect(items[0]).toHaveTextContent("Repeat 1");
  expect(items[0].getAttribute("data-color")).not.toBe(items[1].getAttribute("data-color"));
});

test("a standstill pause shows each qualifying rest against the required count", async () => {
  render(<PausePanel tune={pausedTune("StandstillBaselineUnassessable")} step={0} busy={false} onDecide={() => undefined} />);
  expect(await screen.findByText(/2 of 3 rests qualified/)).toBeInTheDocument();
  expect(screen.getByText("3.500")).toBeInTheDocument();
});

test("disables the buttons while a request is in flight", () => {
  render(<PausePanel tune={pausedTune()} step={0} busy={true} onDecide={() => undefined} />);
  for (const name of ["Continue past this rung", "Re-run this rung", "Stop the ladder"]) {
    expect(screen.getByRole("button", { name })).toBeDisabled();
  }
});

test("a failed fetch is reported on one line", async () => {
  getRun.mockRejectedValue(new Error("boom"));
  render(<PausePanel tune={pausedTune()} step={0} busy={false} onDecide={() => undefined} />);
  expect(await screen.findByText(/boom/)).toBeInTheDocument();
});
