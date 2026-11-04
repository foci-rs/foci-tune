import { expect, test } from "vitest";
import type { ProbePlot } from "@/api";
import { ensembleOptions } from "./EnsemblePlot";
import type { OverlayRun } from "./TracePanels";

const response = (fit: boolean) => ({ edges: 5, mean: [0, 1], fit: fit ? [0, 1] : null });
const run = (label: string, fit = true): OverlayRun<ProbePlot> => ({
  id: label,
  label,
  color: "#fff",
  value: {
    t_s: [0, 1],
    velocity: response(fit),
    torque: response(fit),
    position_error: response(fit),
  } as unknown as ProbePlot,
});

test.each([
  ["velocity", "Speed response (1 = full step)"],
  ["torque", "Torque response (relative to the step)"],
  ["position_error", "Position error (relative to the step)"],
] as const)("the %s tab's vertical axis says what it shows", (signal, label) => {
  const options = ensembleOptions([run("Repeat 1")], signal);
  expect(options.axes?.[1].label).toBe(label);
  expect(options.axes?.[0].label).toBe("Time after the step (ms)");
});

test("each repeat has one legend entry; its fitted curve is drawn but not listed", () => {
  const options = ensembleOptions([run("Repeat 1"), run("Repeat 2")], "velocity");
  const [, ...series] = options.series ?? [];
  expect(series.map((s) => s.label)).toEqual(["Repeat 1", "Repeat 1 fit", "Repeat 2", "Repeat 2 fit"]);
  expect(series.map((s) => s.class)).toEqual([undefined, "legend-hidden", undefined, "legend-hidden"]);
  expect(series[1].dash).toBeDefined();
});

test("the legend lists names only, without the cursor's live values or the time entry", () => {
  expect(ensembleOptions([run("Repeat 1")], "velocity").legend?.live).toBe(false);
});

test("a repeat with no fitted curve has just its mean", () => {
  const [, ...series] = ensembleOptions([run("Repeat 1", false)], "velocity").series ?? [];
  expect(series.map((s) => s.label)).toEqual(["Repeat 1"]);
});
