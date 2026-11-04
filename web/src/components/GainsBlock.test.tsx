import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { ControlSnapshot, Filters } from "@/api";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { LadderInput } from "@/tune";
import { FiltersCard, GainsCard, RunControls } from "./GainsBlock";

const live = {
  torque_p: 300,
  torque_i: 20,
  flux_p: 310,
  flux_i: 21,
} as ControlSnapshot;
const ladder: LadderInput = { axis: "velocity_p", text: "", repeats: 2, floor: 0.05, marginal: 0.08 };
const filters: Filters = { velocity_hz: 200, torque_hz: 1000, position_hz: 300, flux_hz: 1100 };

function gainsCard(over: Partial<Parameters<typeof GainsCard>[0]> = {}) {
  return (
    <TooltipProvider>
      <GainsCard
        gains={{ velocity_p: 841, velocity_i: 412, position_p: 120 }}
        positionI={4}
        live={live}
        pwmHz={50_000}
        disabled={false}
        ladder={ladder}
        ladderOn={false}
        ladderDefaults={[]}
        canLoadLive
        onGains={() => {}}
        onLoadLive={() => {}}
        onLadder={() => {}}
        {...over}
      />
    </TooltipProvider>
  );
}

test("gains run Torque, Flux, Velocity, Position; only velocity and position are editable", () => {
  const onGains = vi.fn();
  render(gainsCard({ onGains }));
  const rows = within(screen.getByRole("table", { name: "Gains" })).getAllByRole("row");
  expect(rows.slice(1).map((r) => r.querySelector("th")?.textContent)).toEqual([
    "Torque",
    "Flux",
    "Velocity",
    "Position",
  ]);
  expect(rows[1]).toHaveTextContent("300");
  expect(rows[1]).toHaveTextContent("20");
  expect(within(rows[1]).queryByRole("spinbutton")).toBeNull();
  expect(within(rows[2]).queryByRole("spinbutton")).toBeNull();
  expect(within(rows[3]).getAllByRole("spinbutton")).toHaveLength(2);
  expect(within(rows[4]).getAllByRole("spinbutton")).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: "Increase Velocity P by 50 raw" }));
  expect(onGains).toHaveBeenCalledWith({ velocity_p: 891, velocity_i: 412, position_p: 120 });
});

test("the raw velocity I sits in a tooltip on the Hz input, not beside it", async () => {
  render(gainsCard());
  const row = screen.getByRole("row", { name: /Velocity/ });
  expect(row).not.toHaveTextContent("raw 412");
  fireEvent.focus(within(row).getByRole("spinbutton", { name: "Velocity I corner (Hz)" }));
  expect(await screen.findByRole("tooltip")).toHaveTextContent("Raw value 412, as written to the config");
});

test("Position I looks like the other gains but is locked, with no tooltip", () => {
  render(gainsCard());
  const row = screen.getByRole("row", { name: /Position/ });
  expect(within(row).getByText("Position I")).toBeInTheDocument();
  const input = within(row).getByRole("spinbutton", { name: "Position I (raw)" });
  expect(input).toHaveValue(4);
  expect(input).toHaveAttribute("readonly");
  expect(within(row).getByRole("button", { name: "Increase Position I" })).toBeDisabled();
  expect(within(row).getByRole("button", { name: "Decrease Position I" })).toBeDisabled();
  fireEvent.focus(input);
  expect(screen.queryByRole("tooltip")).toBeNull();
});

test("the Gains card loads only the live gains, from an icon button without a tooltip", () => {
  const onLoadLive = vi.fn();
  render(gainsCard({ onLoadLive }));
  const button = screen.getByRole("button", { name: "Load live gains" });
  expect(button).toHaveTextContent("");
  fireEvent.focus(button);
  expect(screen.queryByRole("tooltip")).toBeNull();
  fireEvent.click(button);
  expect(onLoadLive).toHaveBeenCalledTimes(1);
});

test("the Filters card has its own icon button that loads only the live filters", () => {
  const onLoadLive = vi.fn();
  render(
    <TooltipProvider>
      <FiltersCard filters={filters} disabled={false} canLoadLive onFilters={() => {}} onLoadLive={onLoadLive} />
    </TooltipProvider>,
  );
  const button = screen.getByRole("button", { name: "Load live filters" });
  fireEvent.click(button);
  expect(onLoadLive).toHaveBeenCalledTimes(1);
});

test("the live buttons are disabled while there is nothing to load", () => {
  render(
    <TooltipProvider>
      <FiltersCard filters={filters} disabled={false} canLoadLive={false} onFilters={() => {}} onLoadLive={() => {}} />
    </TooltipProvider>,
  );
  expect(screen.getByRole("button", { name: "Load live filters" })).toBeDisabled();
});

test("filters run Torque, Flux, Velocity, Position", () => {
  render(
    <TooltipProvider>
      <FiltersCard filters={filters} disabled={false} canLoadLive onFilters={() => {}} onLoadLive={() => {}} />
    </TooltipProvider>,
  );
  const card = screen.getByRole("region", { name: "Filters" });
  expect(
    within(card)
      .getAllByRole("spinbutton")
      .map((i) => i.getAttribute("aria-label")),
  ).toEqual(["Torque filter (Hz)", "Flux filter (Hz)", "Velocity filter (Hz)", "Position filter (Hz)"]);
});

test("Run is disabled and lists every failing reason", () => {
  const onRun = vi.fn();
  render(
    <RunControls
      label="Run"
      blocked={["axis x is not homed", "the trace interface of the stepper's board is not open"]}
      busy={false}
      onRun={onRun}
    />,
  );
  const button = screen.getByRole("button", { name: "Run" });
  expect(button).toBeDisabled();
  fireEvent.click(button);
  expect(onRun).not.toHaveBeenCalled();
  const reasons = within(screen.getByRole("list", { name: "Why the run cannot start" })).getAllByRole("listitem");
  expect(reasons.map((r) => r.textContent)).toEqual([
    "axis x is not homed",
    "the trace interface of the stepper's board is not open",
  ]);
});

test("Run is enabled with no reasons and runs once per press", () => {
  const onRun = vi.fn();
  render(<RunControls label="Run" blocked={[]} busy={false} onRun={onRun} />);
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  expect(onRun).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("list")).toBeNull();
});

test("the number inputs and the stepper buttons are the shared shadcn controls", () => {
  render(
    <TooltipProvider>
      <FiltersCard filters={filters} disabled={false} canLoadLive onFilters={() => {}} onLoadLive={() => {}} />
    </TooltipProvider>,
  );
  for (const input of screen.getAllByRole("spinbutton")) {
    expect(input).toHaveAttribute("data-slot", "input");
  }
  const plus = screen.getByRole("button", { name: "Increase Torque filter by 50 Hz" });
  expect(plus).toHaveAttribute("data-slot", "button");
  expect(plus).toHaveAttribute("data-variant", "outline");
});
