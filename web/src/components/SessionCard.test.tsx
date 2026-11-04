import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { SessionView } from "@/api";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SessionCard } from "./SessionCard";

const view = {
  id: "20261102-aaa",
  error: null,
  session: {
    format_version: 1,
    name: "20261102-aaa",
    created_unix_s: 1_793_600_000,
    stepper: "stepper_x",
    kind: { Probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 } },
    pattern: { start: [50, 50], end: [250, 60] },
    gains: { velocity_p: 841, velocity_i: 412, position_p: 120, position_i: 4 },
    filters: { velocity_hz: 200, torque_hz: 1000, position_hz: 300, flux_hz: 1100 },
    ladder: null,
  },
  runs: [],
} as unknown as SessionView;

const show = (over: Partial<Parameters<typeof SessionCard>[0]> = {}) =>
  render(
    <TooltipProvider>
      <SessionCard view={view} canRestore onRestore={() => {}} notes={[]} {...over} />
    </TooltipProvider>,
  );

test("the card names the session, summarises its setup and starting gains", () => {
  show();
  const card = screen.getByRole("region", { name: "Session 20261102-aaa" });
  expect(card).toHaveTextContent("stepper_x");
  expect(card).toHaveTextContent("Probe");
  expect(card).toHaveTextContent("(50, 50) to (250, 60)");
  expect(card).toHaveTextContent("Started with");
  expect(card).toHaveTextContent("Velocity P 841");
  expect(card).toHaveTextContent("Position P 120");
});

test("Restore starting values lives on the session card and follows canRestore", () => {
  const onRestore = vi.fn();
  const { rerender } = show({ onRestore });
  fireEvent.click(screen.getByRole("button", { name: "Restore starting values" }));
  expect(onRestore).toHaveBeenCalledTimes(1);
  rerender(
    <TooltipProvider>
      <SessionCard view={view} canRestore={false} onRestore={onRestore} notes={[]} />
    </TooltipProvider>,
  );
  expect(screen.getByRole("button", { name: "Restore starting values" })).toBeDisabled();
});

test("no session means no card", () => {
  const { container } = show({ view: { ...view, session: null } as SessionView });
  expect(container).toBeEmptyDOMElement();
});

test("what happened to the session's runs is listed under the summary", () => {
  show({
    notes: [
      "The run at 25 Hz was rejected: stepper_y moved while stepper_x was being tested, so the result can't be trusted.",
      "The sweep stopped after 25 Hz, so 50, 75 and 100 Hz did not run.",
    ],
  });
  const card = screen.getByRole("region", { name: "Session 20261102-aaa" });
  expect(within(card).getByText(/stepper_y moved while stepper_x was being tested/)).toBeInTheDocument();
  expect(within(card).getByText(/The sweep stopped after 25 Hz/)).toBeInTheDocument();
});
