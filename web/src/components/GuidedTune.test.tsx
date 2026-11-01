import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
      gates: { damping_floor: 0.05, marginal: 0.08, standstill_factor: 2 },
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

beforeEach(() => {
  posts = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        posts.push(path);
        return new Response("{}");
      }
      if (path === "/api/tunes") {
        return new Response(JSON.stringify([{ id: "t1", stepper: "stepper_x", state: current.state, created: 1, current_step: "VelocityP" }]));
      }
      return new Response(JSON.stringify(current));
    }),
  );
});
afterEach(() => vi.unstubAllGlobals());

const printer = { steppers: [], preconditions: [], moonraker: { max_velocity: 500 } } as unknown as PrinterView;

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

test("preparing a step posts the request and never the confirmation", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Prepare the step" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/steps"]));
  expect(screen.queryByRole("button", { name: "Start the step" })).not.toBeInTheDocument();
});

test("a locked tune shows the reason with only restore and acknowledge", async () => {
  current = detail("AwaitingChoice", "the restore has not run");
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByText("the restore has not run")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Restore the starting gains" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Acknowledge" })).toBeEnabled();
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
