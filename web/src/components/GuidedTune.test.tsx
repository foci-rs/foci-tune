import { fireEvent, render as renderPlain, screen, waitFor, within } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { PrinterView, Tune, TuneDetail, TuneState } from "@/api";
import { GuidedTune } from "./GuidedTune";

const render = (ui: React.ReactElement) => renderPlain(<TooltipProvider>{ui}</TooltipProvider>);

vi.mock("./GuidedSetup", () => ({
  GuidedSetup: (p: { tune?: unknown; readOnly?: boolean; actions?: React.ReactNode }) => (
    <div>
      <p>{`setup form ${p.tune === undefined ? "new" : p.readOnly ? "read-only" : "editable"}`}</p>
      {!p.readOnly && p.actions}
    </div>
  ),
}));
let feedConnected: boolean | undefined;
let feedListener: ((m: { type: string; reason?: string }) => void) | null = null;
vi.mock("@/feed", () => ({
  useFeed: () => ({
    connected: feedConnected,
    listen: (fn: typeof feedListener) => {
      feedListener = fn;
      return () => undefined;
    },
  }),
}));

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
            octave_divisions: 3,
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
let postRefused: string | null = null;
let refusePath: string | null = null;
let onPost: ((path: string) => void) | null = null;
let listed: { id: string; state: TuneState }[] | null = null;
let posts: string[];
let bodies: unknown[];

beforeEach(() => {
  posts = [];
  bodies = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit) => {
      if (init?.method === "POST" && postRefused !== null && (refusePath === null || path.endsWith(refusePath))) {
        return new Response(JSON.stringify({ error: postRefused }), { status: path.endsWith("/steps") ? 400 : 409 });
      }
      if (init?.method === "POST") {
        posts.push(path);
        onPost?.(path);
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
        const rows = listed ?? [{ id: "t1", state: current.state }];
        return new Response(
          JSON.stringify(
            rows.map((r, i) => ({
              id: r.id,
              stepper: "stepper_x",
              state: r.state,
              created: 10 - i,
              current_step: "VelocityP",
            })),
          ),
        );
      }
      return new Response(JSON.stringify(current));
    }),
  );
});
afterEach(() => {
  listed = null;
  postRefused = null;
  refusePath = null;
  onPost = null;
  feedListener = null;
  feedConnected = undefined;
  vi.unstubAllGlobals();
});

const printer = {
  steppers: [{ name: "stepper_x", serial: "s1", channel: 0 }],
  boards: [
    {
      serial: "s1",
      channels: [{ channel: 0, control: { velocity_p: 1000, velocity_i: 50, position_p: 800, pwm_hz: 50000 } }],
    },
  ],
  preconditions: [],
  moonraker: { max_velocity: 500 },
  engine: { current: null, queue: [] },
} as unknown as PrinterView;

test("an announced step shows its settings locked and one sentence, and only Run starts it", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  const run = await screen.findByRole("button", { name: "Run" });
  expect(screen.getByRole("spinbutton", { name: "Seed" })).toBeDisabled();
  expect(screen.getByRole("combobox", { name: "Lowest step" })).toBeDisabled();
  expect(screen.getByText("Runs:").parentElement).toHaveTextContent(/^Runs: \d+$/);
  expect(screen.queryByText(/duration/i)).toBeNull();
  expect(screen.queryByText(/machine will move/i)).toBeNull();
  expect(screen.queryByText("Announcement")).toBeNull();
  expect(screen.queryByRole("table", { name: /results/ })).toBeNull();
  expect(posts).toEqual([]);
  fireEvent.click(run);
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/steps/confirm"]));
});

test("Adjust discards the prepared step and opens the form again with the same settings", async () => {
  current = detail("Announced");
  onPost = (path) => {
    if (path.endsWith("/steps/cancel")) current = detail("Ready", null, false);
  };
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Adjust" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/steps/cancel"]));
  const seed = await screen.findByRole("spinbutton", { name: "Seed" });
  await waitFor(() => expect(seed).toBeEnabled());
  expect(screen.getByRole("spinbutton", { name: "Repeats" })).toHaveValue(2);
  expect(screen.getByRole("combobox", { name: "Lowest step" })).toHaveTextContent("-1");
  expect(screen.getByRole("button", { name: "Run" })).toBeInTheDocument();
});

test("Run prepares the step from the live seed and starts it, in that order", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Run" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/steps", "/api/tunes/t1/steps/confirm"]));
  expect(bodies[0]).toMatchObject({ seed: "live", repeats: 3 });
});

test("if the service refuses the start, the prepared step is discarded and the form stays open with the reason", async () => {
  current = detail("Ready", null, false);
  refusePath = "/steps/confirm";
  postRefused = "the printer is not fully homed";
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Run" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "The printer isn't fully homed. Home X, Y and Z, then try again.",
  );
  expect(posts).toEqual(["/api/tunes/t1/steps", "/api/tunes/t1/steps/cancel"]);
  expect(screen.getByRole("spinbutton", { name: "Seed" })).toBeEnabled();
});

test("if the step cannot even be prepared, nothing is discarded", async () => {
  current = detail("Ready", null, false);
  refusePath = "/steps";
  postRefused = "The seed can't be zero. Use a value above zero, or the live value.";
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Run" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("The seed can't be zero");
  expect(posts).toEqual([]);
});

test("a locked tune shows the reason with only restore", async () => {
  current = detail("AwaitingChoice", "the restore has not run");
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByText("the restore has not run")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Restore the starting gains" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "Acknowledge" })).not.toBeInTheDocument();
  for (const name of [/abandon/i, /accept/i, /^run$/i, /extend/i, /use this/i]) {
    expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
  }
});

const scoredStep = () => {
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
  const done = { zeta: { Assessed: 0.1 }, p99_by_speed: [3, 4], worst_torque_sd: 1 };
  current.steps[0].rungs.forEach((r) => (r.repeat_values = [done, done]));
  current.steps[0].assessment = {
    verdicts: [none, none, none],
    medians: [medians, medians, medians],
    halt: null,
    best: 1,
    runner_up: null,
    tie: [1],
    spreads: [null, null, null],
  };
};

test("choosing is offered in the bar once the step awaits a choice", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "More ways to choose" });
  expect(screen.queryByRole("button", { name: "Run" })).not.toBeInTheDocument();
});

test("Accept takes the recommended gain, named on the button", async () => {
  current = detail("AwaitingChoice");
  scoredStep();
  current.steps[0].recommendation = { choice: { Measured: { rung: 1 } }, extend: null };
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Accept 1000" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/choice"]));
  expect(bodies[0]).toEqual({ choice: "recommended" });
});

test("a measured gain is picked from the menu", async () => {
  current = detail("AwaitingChoice");
  scoredStep();
  current.steps[0].recommendation = { choice: { Measured: { rung: 1 } }, extend: null };
  render(<GuidedTune printer={printer} />);
  fireEvent.keyDown(await screen.findByRole("button", { name: "More ways to choose" }), { key: "Enter" });
  fireEvent.click(screen.getByRole("menuitem", { name: "1260" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/choice"]));
  expect(bodies[0]).toEqual({ choice: { rung: 2 } });
});

test("a typed gain is sent from the dialog, which starts at the recommendation", async () => {
  current = detail("AwaitingChoice");
  scoredStep();
  current.steps[0].recommendation = { choice: { Interpolated: { shown: 1100.4, between: [1, 2] } }, extend: null };
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByRole("button", { name: "Accept 1100" })).toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("button", { name: "More ways to choose" }), { key: "Enter" });
  fireEvent.click(screen.getByRole("menuitem", { name: "Enter a value…" }));
  const input = screen.getByRole("spinbutton", { name: "Gain" });
  expect(input).toHaveValue(1100);
  fireEvent.change(input, { target: { value: "950" } });
  fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/choice"]));
  expect(bodies[0]).toEqual({ choice: { value: 950 } });
});

test("when the best gain is at an edge the bar offers a lower or higher one, and the card says what it would try", async () => {
  current = detail("AwaitingChoice");
  scoredStep();
  current.steps[0].assessment!.best = 0;
  current.steps[0].recommendation = { choice: { Measured: { rung: 0 } }, extend: "Below" };
  render(<GuidedTune printer={printer} />);
  const extend = await screen.findByRole("button", { name: "Try a lower gain" });
  const names = within(extend.closest(".sticky") as HTMLElement)
    .getAllByRole("button")
    .map((b) => b.textContent);
  expect(names).toEqual(["Abandon tune", "Try a lower gain", "Accept 800", ""]);
  expect(
    screen.getByText(/is at the low end of what was tried, so a lower gain may be better\. Try \d+: 8 more runs\./),
  ).toBeInTheDocument();
  fireEvent.click(extend);
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/extension"]));
  expect(bodies[0]).toEqual({ direction: "below" });
});

test("there is no extension button when the best gain is inside the range", async () => {
  current = detail("AwaitingChoice");
  scoredStep();
  current.steps[0].recommendation = { choice: { Measured: { rung: 1 } }, extend: null };
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Accept 1000" });
  expect(screen.queryByRole("button", { name: /Try a (lower|higher) gain/ })).toBeNull();
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

test("the card keeps the drift lines but no recommendation or comparison sentence, which the table already shows", async () => {
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
  current.steps[0].recommendation = { choice: { Measured: { rung: 1 } }, extend: null };
  current.steps[0].drift = [{ metric: "p99 100", first: 4, rerun: 9, verdict: "Drifted" }];
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByText(/may depend on run order/i)).toBeInTheDocument();
  expect(screen.queryByText(/^Recommended gain/)).toBeNull();
  expect(screen.queryByText(/too close to tell apart|next best gain|nothing to compare/)).toBeNull();
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
  await screen.findByRole("button", { name: "Stop" });
  expect(screen.queryByRole("button", { name: "More ways to choose" })).not.toBeInTheDocument();
});

test("a pause decision is posted through the tune's decision route", async () => {
  current = detail("Paused");
  current.steps[0].paused = { rung: 0, reason: "DampingUnassessable", unix_ms: 1 };
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: "Stop" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/decision"]));
  expect(bodies[0]).toEqual({ decision: "stop" });
});

test("the header marks the axis being tuned and Setup shows the tune's settings instead", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "More ways to choose" });
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
  expect(screen.getByText("setup form read-only")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "More ways to choose" })).not.toBeInTheDocument();
});

test("an axis that has no step yet says so", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "More ways to choose" });
  fireEvent.click(screen.getByRole("button", { name: /Position P/ }));
  expect(screen.getByText(/not reached yet/i)).toBeInTheDocument();
});

test("the result step explains that there is no result before the tune is done", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "More ways to choose" });
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

test("the bar runs the step, and says why it cannot while the form is wrong", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  const prepare = await screen.findByRole("button", { name: "Run" });
  expect(prepare.closest(".sticky")).not.toBeNull();
  fireEvent.change(screen.getByRole("spinbutton", { name: "Repeats" }), { target: { value: "0" } });
  expect(prepare).toBeDisabled();
  expect(
    within(prepare.closest(".sticky") as HTMLElement).getAllByText(/repeat at least once/i).length,
  ).toBeGreaterThan(0);
});

test("an announced step is started from the bar", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  const start = await screen.findByRole("button", { name: "Run" });
  expect(start.closest(".sticky")).not.toBeNull();
});

test("the next-step card shows the seed, the range as dropdowns and the ladder they give", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Run" });
  expect(screen.getByRole("spinbutton", { name: "Seed" })).toHaveValue(1000);
  expect(screen.getByRole("combobox", { name: "Lowest step" })).toBeInTheDocument();
  expect(screen.getByRole("combobox", { name: "Highest step" })).toBeInTheDocument();
  expect(
    within(within(screen.getByRole("table", { name: "Ladder" })).getAllByRole("row")[0]).getAllByRole("cell"),
  ).toHaveLength(7);
  expect(screen.queryByText(/rung/i)).toBeNull();
});

test("a typed seed and a changed range are what Run sends", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  fireEvent.change(await screen.findByRole("spinbutton", { name: "Seed" }), { target: { value: "1200" } });
  fireEvent.click(screen.getByRole("combobox", { name: "Highest step" }));
  fireEvent.click(screen.getByRole("option", { name: /^5 / }));
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/steps"]));
  expect(bodies[0]).toEqual({ seed: { typed: 1200 }, repeats: 3, lower_k: -3, upper_k: 5, octave_divisions: 3 });
});

test("reload puts the live seed back and Run sends live again", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  fireEvent.change(await screen.findByRole("spinbutton", { name: "Seed" }), { target: { value: "1200" } });
  fireEvent.click(screen.getByRole("button", { name: "Use the live value" }));
  expect(screen.getByRole("spinbutton", { name: "Seed" })).toHaveValue(1000);
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  await waitFor(() => expect(bodies[0]).toMatchObject({ seed: "live" }));
});

test("without a live value Run waits for a seed and says so", async () => {
  current = detail("Ready", null, false);
  const silent = { ...printer, boards: [] } as unknown as PrinterView;
  render(<GuidedTune printer={silent} />);
  const next = await screen.findByRole("button", { name: "Run" });
  expect(next).toBeDisabled();
  expect(
    within(next.closest(".sticky") as HTMLElement).getAllByText(/no live value.*enter a seed/i).length,
  ).toBeGreaterThan(0);
  fireEvent.change(screen.getByRole("spinbutton", { name: "Seed" }), { target: { value: "900" } });
  expect(next).toBeEnabled();
});

test("a new tune shows the steps with Setup open, and the later ones wait for the tune", async () => {
  current = detail("Done");
  listed = [{ id: "t1", state: "Done" }];
  render(<GuidedTune printer={printer} />);
  await screen.findByText("setup form new");
  const steps = within(screen.getByRole("navigation", { name: "Steps" }));
  expect(steps.getByRole("button", { name: /Setup/ })).toHaveAttribute("aria-current", "step");
  fireEvent.click(steps.getByRole("button", { name: /Position P/ }));
  expect(screen.getByText(/create the tune first/i)).toBeInTheDocument();
  expect(screen.getByText("setup form new")).not.toBeVisible();
  fireEvent.click(steps.getByRole("button", { name: /Setup/ }));
  expect(screen.getByText("setup form new")).toBeVisible();
});

test("finished and abandoned tunes are not listed: with only those, the page opens a new tune", async () => {
  current = detail("Done");
  listed = [
    { id: "t1", state: "Done" },
    { id: "t2", state: "Abandoned" },
  ];
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByText("setup form new")).toBeInTheDocument();
});

test("a finished tune opened by link shows its steps and result", async () => {
  current = detail("Done");
  listed = [{ id: "t1", state: "Done" }];
  render(<GuidedTune printer={printer} initialTune="t1" />);
  expect(await screen.findByText(/no result yet/i)).toBeInTheDocument();
});

test("before any step the Setup step is the editable form, with Abandon beside Next", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Run" });
  fireEvent.click(within(screen.getByRole("navigation", { name: "Steps" })).getByRole("button", { name: /Setup/ }));
  expect(screen.getByText("setup form editable")).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "Abandon tune" })).toHaveLength(1);
});

test("once a step exists the Setup step is read-only and the bar keeps Abandon", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Run" });
  fireEvent.click(within(screen.getByRole("navigation", { name: "Steps" })).getByRole("button", { name: /Setup/ }));
  expect(screen.getByText("setup form read-only")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Abandon tune" })).toBeInTheDocument();
});

test("a feed resync refreshes the tune, so its buttons match what the service holds", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Run" });
  current = detail("Abandoned");
  feedListener?.({ type: "resync", reason: "reconnected" });
  await waitFor(() => expect(screen.queryByRole("button", { name: "Run" })).not.toBeInTheDocument());
});

test("abandoning a tune that is already over explains it instead of repeating the server", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  const abandon = await screen.findByRole("button", { name: "Abandon tune" });
  current = detail("Abandoned");
  postRefused = "the tune is already over";
  fireEvent.click(abandon);
  expect(await screen.findByText(/already abandoned, so there is nothing to abandon/i)).toBeInTheDocument();
  expect(screen.queryByText("the tune is already over")).not.toBeInTheDocument();
});

test("while foci-tune is unreachable the actions are disabled and the bar says why", async () => {
  current = detail("Announced");
  feedConnected = false;
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByRole("button", { name: "Run" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Abandon tune" })).toBeDisabled();
  expect(screen.getAllByText("Reconnecting to foci-tune.").length).toBeGreaterThan(0);
});

test("a tune that fails to load says why and can be retried", async () => {
  current = detail("Announced");
  const fetchMock = vi.mocked(fetch);
  const real = fetchMock.getMockImplementation()!;
  let fail = true;
  fetchMock.mockImplementation(async (path, init) => {
    if (fail && String(path) === "/api/tunes") throw new TypeError("Failed to fetch");
    return real(path, init);
  });
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByText(/Can't reach foci-tune/)).toBeInTheDocument();
  fail = false;
  fireEvent.click(screen.getByRole("button", { name: "Try again" }));
  expect(await screen.findByRole("button", { name: "Run" })).toBeInTheDocument();
});

test("a failed action is announced as an alert", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  const abandon = await screen.findByRole("button", { name: "Abandon tune" });
  postRefused = "klipper is not ready";
  fireEvent.click(abandon);
  expect(await screen.findByRole("alert")).toHaveTextContent("Klipper isn't ready");
});

test("Run is the rightmost button of the bar, after Abandon", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  const next = await screen.findByRole("button", { name: "Run" });
  const names = within(next.closest(".sticky") as HTMLElement)
    .getAllByRole("button")
    .map((b) => b.textContent);
  expect(names).toEqual(["Abandon tune", "Run"]);
});

test("Run is the rightmost button of the bar, after Abandon and Adjust", async () => {
  current = detail("Announced");
  render(<GuidedTune printer={printer} />);
  const start = await screen.findByRole("button", { name: "Run" });
  const names = within(start.closest(".sticky") as HTMLElement)
    .getAllByRole("button")
    .map((b) => b.textContent);
  expect(names).toEqual(["Abandon tune", "Adjust", "Run"]);
});

test("a chosen step size is sent with the step", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("combobox", { name: "Step size" }));
  fireEvent.click(screen.getByRole("option", { name: "12% (1/6)" }));
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  await waitFor(() => expect(bodies[0]).toMatchObject({ octave_divisions: 6 }));
});

test("the step card is titled by its sweep and says in a sentence what it tries", async () => {
  current = detail("Running");
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByRole("region", { name: "Velocity P sweep" })).toBeInTheDocument();
  expect(
    screen.getByText("3 gains from 800 to 1260, around the starting gain of 1000, each tested 2 times."),
  ).toBeInTheDocument();
  expect(screen.queryByText(/Step 1/)).toBeNull();
  expect(screen.queryByText(/Seed \(raw\)/)).toBeNull();
  expect(screen.queryByText(/^k /)).toBeNull();
});

test("before Next the card already says how many runs the step makes", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  expect((await screen.findByText("Runs:")).parentElement).toHaveTextContent("Runs: 88");
});

test("the read-only Setup step has a Next that goes on to the step the tune is at", async () => {
  current = detail("AwaitingChoice");
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "More ways to choose" });
  fireEvent.click(within(screen.getByRole("navigation", { name: "Steps" })).getByRole("button", { name: /Setup/ }));
  expect(screen.getByText("setup form read-only")).toBeInTheDocument();
  const next = screen.getByRole("button", { name: "Next" });
  expect(
    within(next.closest(".sticky") as HTMLElement)
      .getAllByRole("button")
      .map((b) => b.textContent),
  ).toEqual(["Abandon tune", "Next"]);
  fireEvent.click(next);
  expect(await screen.findByRole("button", { name: "More ways to choose" })).toBeInTheDocument();
  expect(screen.queryByText("setup form read-only")).toBeNull();
});

test("the editable Setup step leaves Next to the setup form, which saves", async () => {
  current = detail("Ready", null, false);
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("button", { name: "Run" });
  fireEvent.click(within(screen.getByRole("navigation", { name: "Steps" })).getByRole("button", { name: /Setup/ }));
  expect(screen.getByText("setup form editable")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
});

test("while a rung is being run its row says Running with its runs, not Pending", async () => {
  current = detail("Running");
  current.steps[0].rungs[0].started_unix_ms = 1;
  current.steps[0].rungs[0].run_ids = ["r1", "r2"];
  render(<GuidedTune printer={printer} />);
  const table = await screen.findByRole("table", { name: /results/ });
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows[0]).toHaveTextContent("Running: 2 of 8 runs");
  expect(rows[1]).toHaveTextContent("Pending");
});

test("the standstill check being off is said once under the table, not in every row", async () => {
  current = detail("Running");
  render(<GuidedTune printer={printer} />);
  const table = await screen.findByRole("table", { name: /results/ });
  expect(within(table).queryByText(/standstill/i)).toBeNull();
  expect(screen.getAllByText(/The standstill noise check is off for this step/)).toHaveLength(1);
});

test("a running step shows the sweep's progress, also between two runs", async () => {
  current = detail("Running");
  current.steps[0].run_count = 24;
  current.steps[0].rungs[0].run_times = [
    { id: "r1", started_unix_ms: 0, ended_unix_ms: 1 },
    { id: "r2", started_unix_ms: 1, ended_unix_ms: 2 },
  ];
  render(<GuidedTune printer={printer} />);
  expect(await screen.findByRole("progressbar", { name: "Sweep progress" })).toHaveAttribute("aria-valuenow", "8");
  expect(screen.getByText("2 of 24 runs")).toBeInTheDocument();
});

test("the rows are numbered", async () => {
  current = detail("Running");
  render(<GuidedTune printer={printer} />);
  const table = await screen.findByRole("table", { name: /results/ });
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["1", "2", "3"]);
});

test("the standstill run of a step reads Measuring while it is in its motion stage", async () => {
  current = detail("Running");
  const standstillRunning = {
    ...printer,
    engine: { current: { session: "c", run: "run1", state: "moving" }, queue: [] },
  } as unknown as PrinterView;
  render(<GuidedTune printer={standstillRunning} />);
  expect(await screen.findByText("Measuring")).toBeInTheDocument();
  expect(screen.queryByText("Moving")).toBeNull();
});

test("the pause decisions are in the bar, after Abandon, with Continue last", async () => {
  current = detail("Paused");
  current.steps[0].paused = { rung: 0, reason: "DampingUnassessable", unix_ms: 1 };
  render(<GuidedTune printer={printer} />);
  const stop = await screen.findByRole("button", { name: "Stop" });
  const bar = stop.closest(".sticky") as HTMLElement;
  expect(
    within(bar)
      .getAllByRole("button")
      .map((b) => b.textContent),
  ).toEqual(["Abandon tune", "Stop", "Rerun", "Continue"]);
  const card = screen.getByRole("region", { name: "Paused" });
  expect(within(card).queryAllByRole("button")).toHaveLength(0);
});

test.each([
  ["Rerun", "rerun"],
  ["Continue", "continue"],
])("%s posts its decision", async (label, decision) => {
  current = detail("Paused");
  current.steps[0].paused = { rung: 0, reason: "DampingUnassessable", unix_ms: 1 };
  render(<GuidedTune printer={printer} />);
  fireEvent.click(await screen.findByRole("button", { name: label }));
  await waitFor(() => expect(posts).toEqual(["/api/tunes/t1/decision"]));
  expect(bodies[0]).toEqual({ decision });
});

test("the tune's event log is not printed above the page: decisions already show in the table", async () => {
  current = detail("Running");
  current.notes = [
    { unix_ms: 1, text: "operator continued past rung 0: damping unassessable" },
    { unix_ms: 2, text: "operator continued past rung 1: damping unassessable" },
  ];
  render(<GuidedTune printer={printer} />);
  await screen.findByRole("table", { name: /results/ });
  expect(screen.queryByText(/operator continued/)).toBeNull();
});
