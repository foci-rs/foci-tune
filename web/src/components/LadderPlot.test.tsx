import { render } from "@testing-library/react";
import { expect, test, vi } from "vitest";

const made = vi.fn();
const setData = vi.fn();
vi.mock("uplot", () => ({
  default: class {
    constructor() {
      made();
    }
    setData(...args: unknown[]) {
      setData(...args);
    }
    setSize() {}
    destroy() {}
  },
}));

const { LadderPlot } = await import("./LadderPlot");

test("a re-render with equal props keeps the chart and only new data redraws it", () => {
  const props = () => ({
    entries: [{ value: 100, metric: 0.2, assessable: true }],
    repeats: 1,
    axisLabel: "Velocity P (raw)",
    metricLabel: "Velocity zeta",
    floor: 0.05,
    marginal: 0.08,
  });
  const { rerender } = render(<LadderPlot {...props()} />);
  rerender(<LadderPlot {...props()} />);
  rerender(<LadderPlot {...props()} />);
  expect(made).toHaveBeenCalledTimes(1);
  rerender(<LadderPlot {...props()} entries={[{ value: 100, metric: 0.3, assessable: true }]} />);
  expect(made).toHaveBeenCalledTimes(1);
  expect(setData).toHaveBeenLastCalledWith([[100], [0.3], [0.3], [0.05], [0.08]]);
});
