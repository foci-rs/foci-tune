import { render } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { PlotOptions } from "./PlotPanel";
import { GuidedCurve } from "./GuidedCurve";

const seen: PlotOptions[] = [];

vi.mock("./PlotPanel", async (orig) => ({
  ...(await orig<typeof import("./PlotPanel")>()),
  PlotPanel: ({ options }: { options: PlotOptions }) => {
    seen.push(options);
    return <div role="img" />;
  },
}));

test("the p99 axis names its unit in a label short enough for the plot height", () => {
  render(<GuidedCurve rows={[]} speeds={[50]} axisLabel="Velocity P" recommended={null} />);
  const label = seen.at(-1)?.axes?.[1]?.label;
  expect(label).toBe("p99 error (\u03bcm)");
  expect(String(label).length).toBeLessThanOrEqual(16);
});
