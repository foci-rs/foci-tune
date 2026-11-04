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

test("the solid and dashed lines are explained once under the graph", () => {
  render(<EnsemblePlot runs={runs} />);
  expect(
    screen.getByText(
      "Solid lines are the measured average, dashed lines the fitted curve. Responses are relative to the size of the step.",
    ),
  ).toBeInTheDocument();
});
