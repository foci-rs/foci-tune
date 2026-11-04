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
      <SessionCard view={view} canRestore onRestore={() => {}} newSessionNext={false} {...over} />
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
      <SessionCard view={view} canRestore={false} onRestore={onRestore} newSessionNext={false} />
    </TooltipProvider>,
  );
  expect(screen.getByRole("button", { name: "Restore starting values" })).toBeDisabled();
});

test("a changed setup is flagged as a new session on the next run", async () => {
  show({ newSessionNext: true });
  const badge = screen.getByText("New session on next run");
  fireEvent.focus(badge);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("Changing the setup starts a new session");
  expect(
    within(screen.getByRole("region", { name: "Session 20261102-aaa" })).getByText("New session on next run"),
  ).toBeInTheDocument();
});

test("no session means no card", () => {
  const { container } = show({ view: { ...view, session: null } as SessionView });
  expect(container).toBeEmptyDOMElement();
});
