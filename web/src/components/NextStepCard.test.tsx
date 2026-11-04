import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { DEFAULT_PREPARE, prepareState, type PrepareForm } from "@/guided";
import { TooltipProvider } from "@/components/ui/tooltip";
import { NextStepCard } from "./NextStepCard";

const control = { velocity_p: 1000, velocity_i: 50, position_p: 800, pwm_hz: 50000 };

function card(form: PrepareForm = DEFAULT_PREPARE, over: Partial<Parameters<typeof NextStepCard>[0]> = {}) {
  return (
    <TooltipProvider>
      <NextStepCard
        axis="VelocityP"
        busy={false}
        form={form}
        state={prepareState("VelocityP", form, control)}
        live={1000}
        onForm={() => {}}
        {...over}
      />
    </TooltipProvider>
  );
}

test("the seed starts at the live value and typing replaces it", () => {
  const onForm = vi.fn();
  render(card(DEFAULT_PREPARE, { onForm }));
  const seed = screen.getByRole("spinbutton", { name: "Seed" });
  expect(seed).toHaveValue(1000);
  fireEvent.change(seed, { target: { value: "1200" } });
  expect(onForm).toHaveBeenCalledWith({ ...DEFAULT_PREPARE, seed: 1200 });
});

test("reload goes back to the live value, and is only on offer once a seed was typed", () => {
  const onForm = vi.fn();
  const { rerender } = render(card(DEFAULT_PREPARE, { onForm }));
  expect(screen.getByRole("button", { name: "Use the live value" })).toBeDisabled();
  const typed = { ...DEFAULT_PREPARE, seed: 1200 };
  rerender(card(typed, { onForm }));
  fireEvent.click(screen.getByRole("button", { name: "Use the live value" }));
  expect(onForm).toHaveBeenCalledWith({ ...typed, seed: null });
});

test("without a live value there is nothing to reload", () => {
  render(card({ ...DEFAULT_PREPARE, seed: 900 }, { live: null }));
  expect(screen.getByRole("button", { name: "Use the live value" })).toBeDisabled();
});

test("the lowest and highest step are dropdowns of whole steps", () => {
  const onForm = vi.fn();
  render(card(DEFAULT_PREPARE, { onForm }));
  expect(screen.getByRole("combobox", { name: "Lowest step" })).toHaveTextContent("-3");
  expect(screen.getByRole("combobox", { name: "Highest step" })).toHaveTextContent("3");
  fireEvent.click(screen.getByRole("combobox", { name: "Highest step" }));
  fireEvent.click(screen.getByRole("option", { name: "5" }));
  expect(onForm).toHaveBeenCalledWith({ ...DEFAULT_PREPARE, upper: 5 });
});

const rowsOf = () =>
  within(screen.getByRole("table", { name: "Ladder" }))
    .getAllByRole("row")
    .map((row) => [
      within(row).getByRole("rowheader").textContent,
      ...within(row)
        .getAllByRole("cell")
        .map((c) => c.textContent),
    ]);

test("the ladder is a table with one column per step: step, gain and change from the seed", () => {
  render(card());
  expect(rowsOf()).toEqual([
    ["Step", "-3", "-2", "-1", "0", "1", "2", "3"],
    ["Gain", "500", "630", "794", "1000", "1260", "1587", "2000"],
    ["vs seed", "-50%", "-37%", "-21%", "0%", "+26%", "+59%", "+100%"],
  ]);
});

test("the seed's own column is marked", () => {
  render(card());
  const marked = within(screen.getByRole("table", { name: "Ladder" })).getAllByText("0", {
    selector: "[data-seed=true]",
  });
  expect(marked).toHaveLength(1);
});

test("the card no longer counts gains and repeats", () => {
  render(card());
  expect(screen.queryByText(/gains,/)).toBeNull();
});

test("a corner ladder is in Hz", () => {
  const form = { ...DEFAULT_PREPARE, seed: 50, lower: 0, upper: 0 };
  render(card(form, { axis: "VelocityICornerHz", state: prepareState("VelocityICornerHz", form, control) }));
  expect(within(screen.getByRole("table", { name: "Ladder" })).getByText("50.0 Hz")).toBeInTheDocument();
  expect(screen.getByText("Hz", { selector: "[data-slot=input-group-addon] *" })).toBeInTheDocument();
});

test("a step that is left out stays as a muted column and the reason is written under the table", () => {
  const form = { ...DEFAULT_PREPARE, seed: 30000, lower: 0, upper: 1 };
  render(card(form));
  expect(rowsOf()).toEqual([
    ["Step", "0", "1"],
    ["Gain", "30000", "left out"],
    ["vs seed", "0%", "+26%"],
  ]);
  expect(screen.getByText("Step 1 is left out because it is outside what the stepper accepts.")).toBeInTheDocument();
});

test("a problem with the seed or range is shown instead of a ladder", () => {
  const form = { ...DEFAULT_PREPARE, seed: 0 };
  render(card(form));
  expect(screen.queryByRole("table", { name: "Ladder" })).toBeNull();
});

test("the reload button says which live value it goes back to", async () => {
  render(card());
  fireEvent.focus(screen.getByRole("button", { name: "Use the live value" }).parentElement as HTMLElement);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("Use the live value: 1000");
});

test("the ladder sits under a separator and a Sweep subtitle, formatted like the setup subtitles", () => {
  render(card());
  const title = screen.getByText("Sweep");
  expect(title).toHaveClass("text-sm", "font-bold", "text-muted-foreground");
  const section = title.closest("[data-slot=ladder]") as HTMLElement;
  expect(section.querySelector("[data-slot=separator]")).not.toBeNull();
  expect(within(section).getByRole("table", { name: "Ladder" })).toBeInTheDocument();
});

test("without a ladder there is no Sweep title", () => {
  render(card({ ...DEFAULT_PREPARE, seed: 0 }));
  expect(screen.queryByText("Sweep")).toBeNull();
});

test("the step size is a dropdown of the offered sizes and reports the choice", () => {
  const onForm = vi.fn();
  render(card(DEFAULT_PREPARE, { onForm }));
  expect(screen.getByRole("combobox", { name: "Step size" })).toHaveTextContent("26% (1/3)");
  fireEvent.click(screen.getByRole("combobox", { name: "Step size" }));
  expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
    "12% (1/6)",
    "19% (1/4)",
    "26% (1/3)",
    "41% (1/2)",
  ]);
  fireEvent.click(screen.getByRole("option", { name: "12% (1/6)" }));
  expect(onForm).toHaveBeenCalledWith({ ...DEFAULT_PREPARE, divisions: 6 });
});

test("the change from the seed follows the step size", () => {
  const form = { ...DEFAULT_PREPARE, divisions: 6 };
  render(card(form, { state: prepareState("VelocityP", form, control) }));
  expect(rowsOf()[2]).toEqual(["vs seed", "-29%", "-21%", "-11%", "0%", "+12%", "+26%", "+41%"]);
  expect(rowsOf()[1]).toEqual(["Gain", "707", "794", "891", "1000", "1122", "1260", "1414"]);
});

test("under the ladder the card says how many runs and how long, as a pair of lines", () => {
  render(card(DEFAULT_PREPARE, { runs: { count: 110, seconds: 1020 } }));
  const list = screen.getByRole("list", { name: "Cost of the step" });
  expect(
    within(list)
      .getAllByRole("listitem")
      .map((li) => li.textContent),
  ).toEqual(["Runs: 110", "Estimated duration: 17 minutes"]);
});

test("an unknown duration says so rather than leaving the line out", () => {
  render(card(DEFAULT_PREPARE, { runs: { count: 110, seconds: null } }));
  expect(within(screen.getByRole("list", { name: "Cost of the step" })).getAllByRole("listitem")[1]).toHaveTextContent(
    "Estimated duration: not known yet",
  );
});

test("without a ladder there are no runs to count", () => {
  render(card({ ...DEFAULT_PREPARE, seed: 0 }, { runs: { count: 0, seconds: null } }));
  expect(screen.queryByRole("list", { name: "Cost of the step" })).toBeNull();
});
