import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { ProbePlot } from "@/api";
import { EnsemblePlot } from "./EnsemblePlot";
import type { OverlayRun } from "./TracePanels";

vi.mock("./PlotPanel", async (orig) => ({
  ...(await orig<typeof import("./PlotPanel")>()),
  PlotPanel: ({ label }: { label: string }) => <div role="img" aria-label={label} />,
}));

const response = { edges: 5, mean: [0, 1], fit: [0, 1] };
const runs = [
  {
    id: "a",
    label: "Repeat 1",
    color: "#fff",
    value: { t_s: [0, 1], velocity: response, torque: response, position_error: response } as unknown as ProbePlot,
  },
] as OverlayRun<ProbePlot>[];

test("the graph views are line-style tabs, starting on the velocity view", () => {
  render(<EnsemblePlot runs={runs} />);
  const tabs = screen.getAllByRole("tab");
  expect(tabs.map((t) => t.textContent)).toEqual(["Velocity", "Torque", "Position error"]);
  expect(screen.getByRole("tablist", { name: "Graph view" })).toHaveAttribute("data-variant", "line");
  expect(screen.getByRole("tab", { name: "Velocity" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("img", { name: "Step-response ensembles, velocity" })).toBeInTheDocument();
});

test("choosing a tab switches the graph", () => {
  render(<EnsemblePlot runs={runs} />);
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Torque" }));
  expect(screen.getByRole("tab", { name: "Torque" })).toHaveAttribute("aria-selected", "true");
  expect(screen.getByRole("img", { name: "Step-response ensembles, torque" })).toBeInTheDocument();
});

test("the caption says what the curves are, what to compare, and that the heights are not micrometres", () => {
  render(<EnsemblePlot runs={runs} />);
  expect(
    screen.getByText(
      "Solid lines are the measured average response to the probe step, dashed lines the fitted curve. foci-tune divides each response by the step size. Compare how far each curve overshoots and how long it takes to settle. The heights are not \u03bcm. The \u03bcm figures in the tables come from the tracking runs.",
    ),
  ).toBeInTheDocument();
});
