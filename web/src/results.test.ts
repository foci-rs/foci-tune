import { expect, test } from "vitest";
import type { Run } from "./api";
import {
  checkLines,
  dampingBar,
  measuredLines,
  outcomeLabel,
  resultColumn,
  runOutcomeReason,
  settingsLines,
} from "./results";

const row = (over: Record<string, number> = {}) => ({
  velocity_p: 841,
  velocity_i: 206,
  position_p: 141,
  velocity_hz: 0,
  torque_hz: 2400,
  position_hz: 0,
  flux_hz: 2400,
  ...over,
});

test("outcomes read as plain words", () => {
  expect(outcomeLabel("done")).toBe("Done");
  expect(outcomeLabel("not_comparable")).toBe("Rejected");
  expect(outcomeLabel("skipped")).toBe("Did not run");
  expect(outcomeLabel("failed")).toBe("Failed");
  expect(outcomeLabel("aborted")).toBe("Cancelled");
  expect(outcomeLabel("killed")).toBe("Stopped");
  expect(outcomeLabel("writing_gains")).toBe("Writing gains");
});

test("the result column is named for what it measures", () => {
  expect(resultColumn("probe")).toEqual({ title: "Damping", unit: "" });
  expect(resultColumn("tracking")).toEqual({ title: "Position error", unit: "μm" });
  expect(resultColumn("standstill")).toEqual({ title: "Torque noise", unit: "" });
});

test("values that never change move to two lines and keep their order", () => {
  const rows = [row({ velocity_i: 206 }), row({ velocity_i: 412 })];
  const s = settingsLines(rows, 50_000);
  expect(s.gains).toBe("Gains: Velocity P 841 · Position P 141");
  expect(s.filters).toBe("Filters: Torque 2400 Hz · Flux 2400 Hz · Velocity 0 Hz · Position 0 Hz");
  expect(s.varying).toEqual(["velocity_i"]);
});

test("velocity I is shown in Hz when it does not change and the PWM rate is known", () => {
  const s = settingsLines([row(), row()], 50_000);
  expect(s.gains).toBe("Gains: Velocity P 841 · Velocity I 25.0 Hz · Position P 141");
  expect(s.varying).toEqual([]);
});

test("values already shown as columns are left out of the lines", () => {
  const s = settingsLines([row(), row()], 50_000, ["velocity_p", "position_p"]);
  expect(s.gains).toBe("Gains: Velocity I 25.0 Hz");
  expect(
    settingsLines([row({ velocity_i: 1 }), row({ velocity_i: 2 })], 50_000, ["velocity_p", "velocity_i", "position_p"])
      .gains,
  ).toBeNull();
});

test("a group with nothing constant has no line, and a single run lists everything", () => {
  const s = settingsLines(
    [row({ velocity_p: 1, torque_hz: 1 }), row({ velocity_p: 2, torque_hz: 2, flux_hz: 7 })],
    null,
  );
  expect(s.filters).toBe("Filters: Velocity 0 Hz · Position 0 Hz");
  expect(s.varying).toEqual(["velocity_p", "torque_hz", "flux_hz"]);
  expect(settingsLines([row()], null).gains).toBe("Gains: Velocity P 841 · Velocity I 206 · Position P 141");
  expect(settingsLines([], null)).toEqual({ gains: null, filters: null, varying: [] });
});

const probeRun = (probe: object | null) =>
  ({
    declared_channel: 0,
    metrics:
      probe === null ? null : { channels: [{ channel: 0, declared: true, tracking: [], standstill: null, probe }] },
    verification: [],
  }) as unknown as Run;

test("a probe is described in plain words", () => {
  const run = probeRun({
    velocity: { status: "fit", zeta: 0.039, damped_hz: 151.6 },
    edges: 134,
    torque_psd_peak_hz: 152.3,
  });
  expect(measuredLines(run, "probe")).toEqual([
    "Damping: 0.039",
    "Ringing frequency: 151.6 Hz",
    "Step responses recorded: 134 (at least 20 are needed)",
    "Torque noise peaks at 152.3 Hz",
  ]);
});

test("a probe whose fit failed says so, and a run without metrics says nothing was measured", () => {
  const failed = probeRun({
    velocity: { status: "failed", reason: "no oscillation" },
    edges: 12,
    torque_psd_peak_hz: null,
  });
  expect(measuredLines(failed, "probe")[0]).toBe("Damping: could not be measured (no oscillation)");
  expect(measuredLines(probeRun(null), "probe")).toEqual(["Nothing was measured."]);
});

test("each check reads as a plain sentence, passed or not", () => {
  const run = {
    verification: [
      { check: "channel_moved", passed: false, detail: "channel 1 moved" },
      { check: "baseline_contiguous", passed: true, detail: "" },
      { check: "no_drop_gaps", passed: true, detail: "" },
      { check: "edge_count", passed: false, detail: "12" },
    ],
  } as unknown as Run;
  expect(checkLines(run, "stepper_x", ["stepper_y"])).toEqual([
    { passed: false, text: "stepper_y moved while stepper_x was being tested." },
    { passed: true, text: "The gains stayed as set." },
    { passed: true, text: "The recording is complete." },
    { passed: false, text: "Too few step responses were recorded to measure damping." },
  ]);
});

test("the reason under an outcome is one plain sentence, or nothing for a good run", () => {
  const entry = (state: string, reason: string | null, failed: string[] = []) => ({
    state,
    ...(reason === null ? {} : { reason }),
    run: { verification: failed.map((check) => ({ check, passed: false, detail: "" })) },
  });
  expect(runOutcomeReason(entry("done", null) as never, "stepper_x", ["stepper_y"])).toBeNull();
  expect(runOutcomeReason(entry("not_comparable", null, ["channel_moved"]) as never, "stepper_x", ["stepper_y"])).toBe(
    "stepper_y moved while stepper_x was being tested, so the result can't be trusted.",
  );
  expect(
    runOutcomeReason(
      entry("skipped", "ladder stopped: damping unassessable at velocity_i_corner_hz 25") as never,
      "x",
      [],
    ),
  ).toBe("The sweep stopped after 25 Hz because its damping could not be measured.");
  expect(runOutcomeReason(entry("failed", "klipper is not ready") as never, "x", [])).toBe("klipper is not ready.");
});

test("the damping bar fills toward a damping of 0.2 and is coloured by the sweep's two limits", () => {
  const low = dampingBar(0.039, 0.05, 0.08);
  expect(low.fraction).toBeCloseTo(0.195, 9);
  expect(low.tone).toBe("low");
  expect(dampingBar(0.06, 0.05, 0.08).tone).toBe("marginal");
  expect(dampingBar(0.1, 0.05, 0.08).fraction).toBeCloseTo(0.5, 9);
  expect(dampingBar(0.1, 0.05, 0.08).tone).toBe("good");
  expect(dampingBar(0.5, 0.05, 0.08).fraction).toBe(1);
  expect(dampingBar(0.05, 0.05, 0.08).tone).toBe("marginal");
});
