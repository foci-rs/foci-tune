import { expect, test } from "vitest";
import type { PrinterView, SessionView } from "./api";
import { sweepNotesFor } from "./sweep";

const run = (id: string, state: string, value: number, verification: object[] = []) => ({
  id,
  state,
  reason: state === "skipped" ? "ladder stopped: damping unassessable at velocity_i_corner_hz 25" : null,
  run: {
    id,
    request: { ladder_value: value },
    verification,
    declared_channel: 0,
    metrics:
      state === "done"
        ? { channels: [{ channel: 0, declared: true, probe: { velocity: { status: "fit", zeta: 0.12 }, edges: 30 } }] }
        : null,
    steps: [{ step: "queued", unix_ms: Number(id.slice(1)) }],
    created_unix_s: 1,
  },
});

const view = (
  runs: object[],
  ladder: object | null = {
    axis: "velocity_i_corner_hz",
    values: [25, 50, 75, 100],
    repeats: 1,
    floor: 0.05,
    marginal: 0.08,
  },
) =>
  ({
    id: "20261104-sweep",
    error: null,
    session: {
      name: "20261104-sweep",
      stepper: "stepper_x",
      kind: { Probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 9 } },
      ladder,
    },
    runs,
  }) as unknown as SessionView;

const printer = {
  steppers: [
    { name: "stepper_x", serial: "AAA", channel: 0 },
    { name: "stepper_y", serial: "AAA", channel: 1 },
  ],
} as unknown as PrinterView;

const rejected = view([
  run("r1", "not_comparable", 25, [{ check: "channel_moved", passed: false, detail: "channel 1 moved" }]),
  run("r2", "skipped", 50),
  run("r3", "skipped", 75),
  run("r4", "skipped", 100),
]);

test("a rejected sweep names what went wrong and which values did not run", () => {
  expect(sweepNotesFor(rejected, printer)).toEqual([
    "The run at 25 Hz was rejected: stepper_y moved while stepper_x was being tested, so the result can't be trusted.",
    "The sweep stopped after 25 Hz, so 50, 75 and 100 Hz did not run.",
  ]);
});

test("without the printer's stepper list the other motor is not named", () => {
  expect(sweepNotesFor(rejected, null)[0]).toContain("Another motor moved while stepper_x was being tested");
});

test("a healthy sweep and a session without a sweep have no notes", () => {
  const healthy = view([run("r1", "done", 25), run("r2", "done", 50)]);
  expect(sweepNotesFor(healthy, printer)).toEqual([]);
  expect(sweepNotesFor(view([run("r1", "done", 25)], null), printer)).toEqual([]);
});
