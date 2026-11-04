import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { Pattern } from "@/api";
import { TooltipProvider } from "@/components/ui/tooltip";
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
  const card = screen.getByRole("region", { name: "Path" });
  expect(within(card).getByRole("alert")).toHaveTextContent("Path refused: pattern leaves the bed");
  expect(within(card).getByRole("note")).toHaveTextContent("toggle spacing too short");
  expect(within(card).queryByRole("button", { name: "Use the proposed pattern" })).toBeNull();
});

test("the dropdown sits between the map and the coordinates", () => {
  render(patternCard());
  const map = screen.getByRole("img");
  const select = screen.getByRole("combobox", { name: "Path" });
  const start = screen.getByRole("group", { name: "Start" });
  expect(map.compareDocumentPosition(select) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(select.compareDocumentPosition(start) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test("Default locks the coordinates and Custom unlocks them", () => {
  const { rerender } = render(patternCard({ mode: "default" }));
  expect(screen.getByRole("combobox", { name: "Path" })).toHaveTextContent("Default");
  for (const input of screen.getAllByRole("spinbutton")) expect(input).toBeDisabled();
  rerender(patternCard({ mode: "custom" }));
  expect(screen.getByRole("combobox", { name: "Path" })).toHaveTextContent("Custom");
  for (const input of screen.getAllByRole("spinbutton")) expect(input).toBeEnabled();
});

test("choosing Custom or Default reports the mode", () => {
  const onMode = vi.fn();
  const { rerender } = render(patternCard({ mode: "default", onMode }));
  fireEvent.click(screen.getByRole("combobox", { name: "Path" }));
  fireEvent.click(screen.getByRole("option", { name: "Custom" }));
  expect(onMode).toHaveBeenCalledWith("custom");
  rerender(patternCard({ mode: "custom", onMode }));
  fireEvent.click(screen.getByRole("combobox", { name: "Path" }));
  fireEvent.click(screen.getByRole("option", { name: "Default" }));
  expect(onMode).toHaveBeenCalledWith("default");
});

test("a refused proposal explains itself and leaves only Custom", () => {
  render(patternCard({ mode: "custom", proposal: null, refusal: "bed limits need stepper_y" }));
  const card = screen.getByRole("region", { name: "Path" });
  expect(card).toHaveTextContent("No default path: bed limits need stepper_y");
  fireEvent.click(screen.getByRole("combobox", { name: "Path" }));
  expect(screen.getByRole("option", { name: "Default" })).toHaveAttribute("aria-disabled", "true");
});

const ladder: LadderInput = { axis: "velocity_p", text: "", repeats: 2, floor: 0.05, marginal: 0.08 };

const sweepCard = (over: Partial<Parameters<typeof LadderCard>[0]> = {}) => (
  <TooltipProvider>
    <LadderCard
      on
      ladder={ladder}
      defaultsFor={() => []}
      errors={{ values: null, runs: null, thresholds: null }}
      probe
      disabled={false}
      onToggle={() => {}}
      onLadder={() => {}}
      {...over}
    />
  </TooltipProvider>
);

test("the stop-rule thresholds are hidden unless the test is a probe", () => {
  const { rerender } = render(sweepCard({ probe: false }));
  expect(screen.queryByText("Stop below damping")).toBeNull();
  expect(screen.queryByText("Mark as marginal below")).toBeNull();
  expect(screen.queryByText(/stop rule applies/)).toBeNull();
  rerender(sweepCard({ probe: true }));
  expect(screen.getByText("Stop below damping")).toBeInTheDocument();
  expect(screen.getByText("Mark as marginal below")).toBeInTheDocument();
});

test("the stop and marginal thresholds explain themselves on focus", async () => {
  render(sweepCard());
  fireEvent.focus(screen.getByText("Stop below damping"));
  expect(await screen.findByRole("tooltip")).toHaveTextContent("stops after a value whose median damping");
  fireEvent.blur(screen.getByText("Stop below damping"));
  fireEvent.focus(screen.getByText("Mark as marginal below"));
  expect(await screen.findByText(/marked as marginal in the results/)).toBeInTheDocument();
});

test("the sweep card is one line while off and shows its setup when on", () => {
  const onToggle = vi.fn();
  const { rerender } = render(sweepCard({ on: false, onToggle }));
  const sw = screen.getByRole("switch", { name: "Gain sweep" });
  expect(sw).toHaveAttribute("aria-checked", "false");
  expect(screen.queryByText("Runs per value")).toBeNull();
  expect(screen.queryByText(/Off:/)).toBeNull();
  fireEvent.click(sw);
  expect(onToggle).toHaveBeenCalledWith(true);
  rerender(sweepCard({ onToggle }));
  expect(screen.getByText("Runs per value")).toBeInTheDocument();
});

test("the gain dropdown has no units, and the values input shows the unit of the chosen gain", () => {
  const { rerender } = render(sweepCard());
  const card = screen.getByRole("region", { name: "Gain sweep" });
  expect(within(card).getByRole("textbox", { name: "Velocity P sweep values" })).toBeInTheDocument();
  expect(within(card).getByText("raw")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("combobox", { name: "Gain to sweep" }));
  expect(screen.getByRole("option", { name: "Velocity I" })).toBeInTheDocument();
  expect(screen.queryByRole("option", { name: /\(Hz\)|\(raw\)/ })).toBeNull();
  fireEvent.keyDown(screen.getByRole("listbox"), { key: "Escape" });
  rerender(sweepCard({ ladder: { ...ladder, axis: "velocity_i_corner_hz" } }));
  expect(within(screen.getByRole("region", { name: "Gain sweep" })).getByText("Hz")).toBeInTheDocument();
});

test("choosing another gain fills in that gain's default values", () => {
  const onLadder = vi.fn();
  render(sweepCard({ onLadder, defaultsFor: (axis) => (axis === "position_p" ? [90, 100] : [1]) }));
  fireEvent.click(screen.getByRole("combobox", { name: "Gain to sweep" }));
  fireEvent.click(screen.getByRole("option", { name: "Position P" }));
  expect(onLadder).toHaveBeenCalledWith({ ...ladder, axis: "position_p", text: "90, 100" });
});

test("validation errors show on the input they belong to", () => {
  const { rerender } = render(sweepCard());
  expect(screen.queryByRole("alert")).toBeNull();
  rerender(
    sweepCard({
      errors: {
        values: "Enter at least one value, separated by commas.",
        runs: "Runs per value must be a whole number from 1 to 10.",
        thresholds: null,
      },
    }),
  );
  const values = screen.getByRole("textbox", { name: "Velocity P sweep values" });
  expect(values).toHaveAttribute("aria-invalid", "true");
  expect(values).toHaveAccessibleDescription("Enter at least one value, separated by commas.");
  const runs = screen.getByRole("spinbutton", { name: "Runs per value" });
  expect(runs).toHaveAttribute("aria-invalid", "true");
  expect(runs).toHaveAccessibleDescription("Runs per value must be a whole number from 1 to 10.");
  expect(screen.getAllByRole("alert")).toHaveLength(2);
});

test("the sweep values are typed next to the dropdown, and a wand inside the input fills in the suggestions", () => {
  const onLadder = vi.fn();
  render(sweepCard({ onLadder, defaultsFor: () => [100, 120, 140] }));
  const input = screen.getByRole("textbox", { name: "Velocity P sweep values" });
  fireEvent.change(input, { target: { value: "90, 110" } });
  expect(onLadder).toHaveBeenCalledWith({ ...ladder, text: "90, 110" });
  const group = input.closest("[data-slot=input-group]") as HTMLElement;
  expect(within(group).getByText("raw")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Defaults" })).toBeNull();
  const wand = within(group).getByRole("button", { name: "Fill in suggested values" });
  fireEvent.click(wand);
  expect(onLadder).toHaveBeenCalledWith({ ...ladder, text: "100, 120, 140" });
});

test("the wand is disabled while there are no suggestions", () => {
  render(sweepCard({ defaultsFor: () => [] }));
  expect(screen.getByRole("button", { name: "Fill in suggested values" })).toBeDisabled();
});

function bar(over: Partial<Parameters<typeof RunBar>[0]> = {}) {
  return (
    <TooltipProvider>
      <RunBar
        runLabel="Run"
        blocked={[]}
        busy={false}
        onRun={() => {}}
        running={false}
        onAbort={() => {}}
        message={null}
        {...over}
      />
    </TooltipProvider>
  );
}

test("a blocked run bar shows the first reason as a sentence and counts the rest", async () => {
  const onRun = vi.fn();
  render(bar({ blocked: ["the printer is not fully homed", "velocity P 99999 is outside 0..=32767"], onRun }));
  expect(screen.getByText("The printer is not fully homed.")).toBeInTheDocument();
  const more = screen.getByText("+1 more");
  fireEvent.focus(more);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("Velocity P 99999 is outside 0..=32767.");
  const run = screen.getByRole("button", { name: "Run" });
  expect(run).toBeDisabled();
  fireEvent.click(run);
  expect(onRun).not.toHaveBeenCalled();
});

test("every reason stays available to screen readers as a list", () => {
  render(bar({ blocked: ["the printer is not fully homed", "no control-state frame yet"] }));
  const reasons = within(screen.getByRole("list", { name: "Why Run is disabled" })).getAllByRole("listitem");
  expect(reasons.map((r) => r.textContent)).toEqual(["the printer is not fully homed", "no control-state frame yet"]);
});

test("a disabled Run explains itself on hover or focus", async () => {
  render(bar({ blocked: ["the printer is not fully homed"] }));
  fireEvent.focus(screen.getByRole("button", { name: "Run" }).parentElement!);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("The printer is not fully homed.");
});

test("an unblocked Run runs once per press and shows no warning", () => {
  const onRun = vi.fn();
  render(bar({ onRun }));
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  expect(onRun).toHaveBeenCalledTimes(1);
  expect(screen.queryByText(/more$/)).toBeNull();
  expect(screen.queryByRole("list")).toBeNull();
});

test("Cancel appears only while a run is going, the message shows, and there is no session note or Restore", () => {
  const onAbort = vi.fn();
  const { rerender } = render(bar());
  expect(screen.queryByRole("button", { name: "Cancel queued runs" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Restore starting values" })).toBeNull();
  expect(screen.queryByText(/new session/)).toBeNull();
  rerender(bar({ running: true, onAbort, message: { tone: "ok", text: "Run queued" } }));
  fireEvent.click(screen.getByRole("button", { name: "Cancel queued runs" }));
  expect(onAbort).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("status")).toHaveTextContent("Run queued");
});
