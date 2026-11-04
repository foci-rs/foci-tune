import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { PrinterView, Tune, TuneDetail, TuneState } from "@/api";
import { GuidedTune } from "./GuidedTune";

vi.mock("@/feed", () => ({ useFeed: () => ({ listen: () => () => undefined }) }));

const gains = { velocity_p: 1000, velocity_i: 50, position_p: 800, position_i: 0 };
const filters = { velocity_hz: 500, torque_hz: 1000, position_hz: 500, flux_hz: 1000 };

function detail(state: TuneState, lock: string | null = null, withStep = true): TuneDetail {
  const tune: Tune = {
    format_version: 1,
    id: "t1",
    setup: {
      stepper: "stepper_x",
      speeds_mm_s: [50, 100],
      pattern: { start: [100, 100], end: [200, 200] },
      probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 },
      tracking_strokes: 2,
      standstill: { rests: 3, dwell_s: 2 },
      gates: { damping_floor: 0.05, marginal: 0.08, standstill_factor: null },
      include_marginal: false,
    },
    snapshot: [gains, filters],
    steps: withStep
      ? [
          {
            axis: "VelocityP",
            seed_raw: 1000,
            repeats: 2,
            lower_k: -1,
            upper_k: 1,
            pwm_hz: 50000,
            base: [gains, filters],
            sessions: { probe: "a", tracking: "b", standstill: "c" },
            rungs: [800, 1000, 1260].map((raw) => ({
              shown: raw,
              raw,
              run_ids: [],
              started_unix_ms: null,
              ended_unix_ms: null,
              skipped: null,
              repeat_values: [],
              run_times: [],
              waived: null,
              superseded: [],
            })),
            notes: [],
            chosen: null,
            chosen_shown: null,
            starts: 0,
            started_unix_ms: null,
            ended_unix_ms: null,
            failed: null,
            assessment: null,
            recommendation: null,
            drift_rung: null,
            drift: [],
            paused: null,
            standstill_gate_off: null,
          },
        ]
      : [],
    state,
    created_unix_s: 1,
    notes: [],
  };
  return { ...tune, config_block: null, lock };
}

let current: TuneDetail;
let posts: string[];
let bodies: unknown[];

beforeEach(() => {
  posts = [];
  bodies = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        posts.push(path);
        bodies.push(typeof init.body === "string" ? JSON.parse(init.body) : init.body);
        return new Response("{}");
      }
      if (path.startsWith("/api/sessions/")) {
        return new Response(
          JSON.stringify({
            id: "a",
            session: null,
            error: null,
            runs: [
              {
                id: "run1",
                state: "moving",
                run: { steps: [{ step: "moving", unix_ms: Date.now() }], predicted_s: 20 },
              },
            ],
          }),
        );
      }
      if (path === "/api/tunes") {
        return new Response(
          JSON.stringify([
            { id: "t1", stepper: "stepper_x", state: current.state, created: 1, current_step: "VelocityP" },
          ]),
        );
      }
      return new Response(JSON.stringify(current));
    }),
  );
});
afterEach(() => vi.unstubAllGlobals());

const printer = {
  steppers: [],
  preconditions: [],
  moonraker: { max_velocity: 500 },
  engine: { current: null, queue: [] },
} as unknown as PrinterView;

test("an announced step states what will happen and only Start confirms it", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  const start = await screen.findByRole("button", { name: "Start the step" });
  expect(screen.getByText(/machine will move/i)).toBeInTheDocument();
  expect(screen.getByText("unknown")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /abandon/i })).toBeInTheDocument();
  expect(posts).toEqual([]);
  fireEvent.click(start);
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/steps/confirm"]));
});

test("the duration estimate uses the run times of the tune's own earlier steps", async () => {
  current = detail("Announced");
  const finished = structuredClone(current.steps[0]);
  finished.rungs[0].run_times = [{ id: "r1", started_unix_ms: 0, ended_unix_ms: 30000 }];
  current.steps = [finished, current.steps[0]];
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Start the step" });
  expect(screen.queryByText("unknown")).not.toBeInTheDocument();
  expect(screen.getByText(/about \d+ min/)).toBeInTheDocument();
});

test("preparing a step posts the request and never the confirmation", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Prepare the step" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/steps"]));
  expect(bodies[0]).toMatchObject({ seed: "live", repeats: 3 });
  expect(screen.queryByRole("button", { name: "Start the step" })).not.toBeInTheDocument();
});

test("a locked tune shows the reason with only restore", async () => {
  current = detail("AwaitingChoice", "the restore has not run");
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByText("the restore has not run")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Restore the starting gains" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "Acknowledge" })).not.toBeInTheDocument();
  for (const name of [/abandon/i, /accept/i, /start the step/i, /extend/i, /use this/i]) {
    expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
  }
});

test("choosing is offered once the step awaits a choice", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Accept the recommendation" });
  expect(screen.queryByRole("button", { name: "Start the step" })).not.toBeInTheDocument();
});

test("the typed value follows the recommendation until the operator types", async () => {
  current = detail("AwaitingChoice");
  const none = {
    inadmissible: null,
    marginal: false,
    unscored: null,
    score: 1,
    deciding_speed: null,
    gap: null,
    tolerance: null,
  };
  const medians = { zeta: 0.1, p99_by_speed: [null, null], worst_torque_sd: 1 };
  current.steps[0].assessment = {
    verdicts: [none, none, none],
    medians: [medians, medians, medians],
    halt: null,
    best: null,
    runner_up: null,
    tie: [],
    spreads: [],
  };
  current.steps[0].recommendation = { choice: { Interpolated: { shown: 1100, between: [1, 2] } }, extend: null };
  render(<GuidedTune printer={printer} />);
  const field = await screen.findByRole("spinbutton", { name: "Value" });
  await waitFor(() => expect(field).toHaveValue(1100));
});

test("a running step with scored rungs does not claim scores can change", async () => {
  current = detail("Running");
  const scored = {
    inadmissible: null,
    marginal: false,
    unscored: null,
    score: 1,
    deciding_speed: null,
    gap: null,
    tolerance: null,
  };
  const open = {
    inadmissible: null,
    marginal: false,
    unscored: null,
    score: null,
    deciding_speed: null,
    gap: null,
    tolerance: null,
  };
  const medians = { zeta: 0.1, p99_by_speed: [null, null], worst_torque_sd: 1 };
  current.steps[0].rungs[0].repeat_values = [{} as never, {} as never];
  current.steps[0].assessment = {
    verdicts: [scored, open, open],
    medians: [medians, medians, medians],
    halt: null,
    best: 0,
    runner_up: null,
    tie: [0],
    spreads: [null, null],
  };
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("table");
  expect(screen.queryByText(/can change as later rungs finish/i)).toBeNull();
});

test("the score column is in micrometres and each scored row shows its deciding speed and gap", async () => {
  current = detail("AwaitingChoice");
  const best = {
    inadmissible: null,
    marginal: false,
    unscored: null,
    score: 2,
    deciding_speed: 1,
    gap: null,
    tolerance: null,
  };
  const behind = {
    inadmissible: null,
    marginal: false,
    unscored: null,
    score: 7.1,
    deciding_speed: 1,
    gap: 5.1,
    tolerance: 3.1,
  };
  const medians = { zeta: 0.1, p99_by_speed: [null, null], worst_torque_sd: 1 };
  current.steps[0].assessment = {
    verdicts: [best, behind, behind],
    medians: [medians, medians, medians],
    halt: null,
    best: 0,
    runner_up: 1,
    tie: [0],
    spreads: [null, null, null],
  };
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("table");
  expect(screen.getByRole("columnheader", { name: /score.*μm/i })).toBeInTheDocument();
  expect(screen.getAllByText(/@ 100 mm\/s/)).not.toHaveLength(0);
  expect(screen.getAllByText(/5\.1 μm.*3\.1 μm/).length).toBeGreaterThan(0);
  expect(screen.getByText(/^best$/i)).toBeInTheDocument();
});

test("the recommendation card carries the comparison line and the drift lines", async () => {
  current = detail("AwaitingChoice");
  const a = {
    inadmissible: null,
    marginal: false,
    unscored: null,
    score: 2,
    deciding_speed: 1,
    gap: null,
    tolerance: null,
  };
  const b = { inadmissible: null, marginal: false, unscored: null, score: 3, deciding_speed: 1, gap: 1, tolerance: 2 };
  const medians = { zeta: 0.1, p99_by_speed: [null, null], worst_torque_sd: 1 };
  current.steps[0].assessment = {
    verdicts: [a, a, b],
    medians: [medians, medians, medians],
    halt: null,
    best: 1,
    runner_up: 2,
    tie: [1, 2],
    spreads: [null, null, null],
  };
  current.steps[0].drift = [{ metric: "p99 100", first: 4, rerun: 9, verdict: "Drifted" }];
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByText(/lower gain/i)).toBeInTheDocument();
  expect(await screen.findByText(/may depend on run order/i)).toBeInTheDocument();
});

test("the drift re-run is announced while the machine is still moving after the last rung", async () => {
  current = detail("Running");
  current.steps[0].drift_rung = { ...current.steps[0].rungs[1], started_unix_ms: 1 };
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByText(/drift check running/i)).toBeInTheDocument();
});

test("a paused tune shows the pause panel and no choice controls", async () => {
  current = detail("Paused");
  current.steps[0].paused = { rung: 0, reason: "DampingUnassessable", unix_ms: 1 };
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Stop the ladder" });
  expect(screen.queryByRole("button", { name: "Accept the recommendation" })).not.toBeInTheDocument();
});

test("a pause decision is posted through the tune's decision route", async () => {
  current = detail("Paused");
  current.steps[0].paused = { rung: 0, reason: "DampingUnassessable", unix_ms: 1 };
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Stop the ladder" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/decision"]));
  expect(bodies[0]).toEqual({ decision: "stop" });
});

test("the header marks the axis being tuned and Setup shows the tune's settings instead", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Accept the recommendation" });
  const steps = within(screen.getByRole("navigation", { name: "Steps" }));
  expect(steps.getAllByRole("button").map((b) => b.textContent)).toEqual([
    "1Setup",
    "2Velocity P",
    "3Velocity I corner",
    "4Position P",
    "5Result",
  ]);
  expect(steps.getByRole("button", { name: /Velocity P/ })).toHaveAttribute("aria-current", "step");
  fireEvent.click(steps.getByRole("button", { name: /Setup/ }));
  expect(screen.getByText("Tracking speeds")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Accept the recommendation" })).not.toBeInTheDocument();
});

test("an axis that has no step yet says so", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Accept the recommendation" });
  fireEvent.click(screen.getByRole("button", { name: /Position P/ }));
  expect(screen.getByText(/not reached yet/i)).toBeInTheDocument();
});

test("the result step explains that there is no result before the tune is done", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Accept the recommendation" });
  fireEvent.click(screen.getByRole("button", { name: /Result/ }));
  expect(screen.getByText(/no result yet/i)).toBeInTheDocument();
});

test("abandoning is offered from the bar", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  const bar = (await screen.findByRole("button", { name: "Abandon tune" })).closest(".sticky");
  expect(bar).not.toBeNull();
});

test("while a run of the step is in flight, the step shows its progress", async () => {
  current = detail("Running");
  const running = {
    ...printer,
    engine: { current: { session: "a", run: "run1", state: "moving" }, queue: [] },
  } as unknown as PrinterView;
  render(<GuidedTune printer={running} />);
  expect(await screen.findByRole("progressbar", { name: "Run progress" })).toBeInTheDocument();
});

test("a run of another session is not shown on this tune's step", async () => {
  current = detail("Running");
  const other = {
    ...printer,
    engine: { current: { session: "zzz", run: "run9", state: "moving" }, queue: [] },
  } as unknown as PrinterView;
  render(<GuidedTune printer={other} />);
  await screen.findByRole("table");
  expect(screen.queryByRole("progressbar", { name: "Run progress" })).not.toBeInTheDocument();
});
