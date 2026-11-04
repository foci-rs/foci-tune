import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { Pattern } from "@/api";
import type { LadderInput } from "@/tune";
import { LadderCard, PatternCard, RunBar } from "./WorkbenchCards";

const limits = { x: [0, 300], y: [0, 300] } as never;
const pattern: Pattern = { start: [50, 50], end: [250, 50] };
const proposal: Pattern = { start: [60, 60], end: [240, 60] };

function patternCard(over: Partial<Parameters<typeof PatternCard>[0]> = {}) {
  return (
    <PatternCard
      limits={limits}
      pattern={pattern}
      proposal={proposal}
      mode="default"
      refusal={null}
      error={null}
      notes={[]}
      disabled={false}
      onPattern={() => {}}
      onMode={() => {}}
      {...over}
    />
  );
}

test("the pattern card reports refusals and notes next to the field", () => {
  render(
    patternCard({
      mode: "custom",
      error: "pattern leaves the bed",
      notes: [{ tone: "warning", text: "toggle spacing too short" }],
    }),
  );
  const card = screen.getByRole("region", { name: "Pattern" });
  expect(within(card).getByRole("alert")).toHaveTextContent("pattern leaves the bed");
  expect(within(card).getByRole("note")).toHaveTextContent("toggle spacing too short");
  expect(within(card).queryByRole("button", { name: "Use the proposed pattern" })).toBeNull();
});

test("the dropdown sits between the map and the coordinates", () => {
  render(patternCard());
  const map = screen.getByRole("img");
  const select = screen.getByRole("combobox", { name: "Pattern" });
  const start = screen.getByRole("group", { name: "Start" });
  expect(map.compareDocumentPosition(select) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(select.compareDocumentPosition(start) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test("Default locks the coordinates and Custom unlocks them", () => {
  const { rerender } = render(patternCard({ mode: "default" }));
  expect(screen.getByRole("combobox", { name: "Pattern" })).toHaveTextContent("Default");
  for (const input of screen.getAllByRole("spinbutton")) expect(input).toBeDisabled();
  rerender(patternCard({ mode: "custom" }));
  expect(screen.getByRole("combobox", { name: "Pattern" })).toHaveTextContent("Custom");
  for (const input of screen.getAllByRole("spinbutton")) expect(input).toBeEnabled();
});

test("choosing Custom or Default reports the mode", () => {
  const onMode = vi.fn();
  const { rerender } = render(patternCard({ mode: "default", onMode }));
  fireEvent.click(screen.getByRole("combobox", { name: "Pattern" }));
  fireEvent.click(screen.getByRole("option", { name: "Custom" }));
  expect(onMode).toHaveBeenCalledWith("custom");
  rerender(patternCard({ mode: "custom", onMode }));
  fireEvent.click(screen.getByRole("combobox", { name: "Pattern" }));
  fireEvent.click(screen.getByRole("option", { name: "Default" }));
  expect(onMode).toHaveBeenCalledWith("default");
});

test("a refused proposal explains itself and leaves only Custom", () => {
  render(patternCard({ mode: "custom", proposal: null, refusal: "bed limits need stepper_y" }));
  const card = screen.getByRole("region", { name: "Pattern" });
  expect(card).toHaveTextContent("No default pattern: bed limits need stepper_y");
  fireEvent.click(screen.getByRole("combobox", { name: "Pattern" }));
  expect(screen.getByRole("option", { name: "Default" })).toHaveAttribute("aria-disabled", "true");
});

const ladder: LadderInput = { axis: "velocity_p", text: "", repeats: 2, floor: 0.05, marginal: 0.08 };

test("the ladder card is one line while off and shows its setup when on", () => {
  const onToggle = vi.fn();
  const { rerender } = render(
    <LadderCard on={false} ladder={ladder} probe disabled={false} onToggle={onToggle} onLadder={() => {}} />,
  );
  const sw = screen.getByRole("switch", { name: "Ladder" });
  expect(sw).toHaveAttribute("aria-checked", "false");
  expect(screen.queryByText("Repeats")).toBeNull();
  fireEvent.click(sw);
  expect(onToggle).toHaveBeenCalledWith(true);
  rerender(<LadderCard on ladder={ladder} probe disabled={false} onToggle={onToggle} onLadder={() => {}} />);
  expect(screen.getByText("Repeats")).toBeInTheDocument();
});

function bar(over: Partial<Parameters<typeof RunBar>[0]> = {}) {
  return (
    <RunBar
      runLabel="Run"
      blocked={[]}
      busy={false}
      onRun={() => {}}
      canRevert={false}
      onRevert={() => {}}
      running={false}
      onAbort={() => {}}
      message={null}
      sessionNote="The first run starts a new session."
      {...over}
    />
  );
}

test("the run bar lists every blocker and keeps Run disabled", () => {
  const onRun = vi.fn();
  render(bar({ blocked: ["axis y is not homed", "velocity P 99999 is outside 0..=65535"], onRun }));
  const run = screen.getByRole("button", { name: "Run" });
  expect(run).toBeDisabled();
  fireEvent.click(run);
  expect(onRun).not.toHaveBeenCalled();
  const reasons = within(screen.getByRole("list", { name: "Why the run cannot start" })).getAllByRole("listitem");
  expect(reasons.map((r) => r.textContent)).toEqual(["axis y is not homed", "velocity P 99999 is outside 0..=65535"]);
});

test("Revert follows the session, Abort appears only while a run is going, and the message shows", () => {
  const onRevert = vi.fn();
  const onAbort = vi.fn();
  const { rerender } = render(bar());
  expect(screen.getByRole("button", { name: "Revert" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: "Abort after this run" })).toBeNull();
  rerender(bar({ canRevert: true, running: true, onRevert, onAbort, message: { tone: "ok", text: "Run queued" } }));
  fireEvent.click(screen.getByRole("button", { name: "Revert" }));
  fireEvent.click(screen.getByRole("button", { name: "Abort after this run" }));
  expect(onRevert).toHaveBeenCalledTimes(1);
  expect(onAbort).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("status")).toHaveTextContent("Run queued");
  expect(screen.getByText("The first run starts a new session.")).toBeInTheDocument();
});
