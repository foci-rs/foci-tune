import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { RungView } from "@/guided";
import { SweepResultsTable } from "./SweepResultsTable";

const row = (index: number, over: Partial<RungView> = {}): RungView => ({
  index,
  shown: 841 + index,
  raw: 841 + index,
  verdict: "Scored",
  skipped: null,
  marginal: false,
  inadmissible: false,
  unscored: false,
  score: 4.2,
  zeta: 0.123,
  p99BySpeed: [3.1, 4.2],
  worstTorqueSd: 1,
  recommended: null,
  repeatsDone: 3,
  decidingSpeed: 1,
  gap: null,
  tolerance: null,
  best: false,
  detail: null,
  ...over,
});

function table(over: Partial<Parameters<typeof SweepResultsTable>[0]> = {}) {
  return (
    <TooltipProvider>
      <SweepResultsTable
        axis="VelocityP"
        label="Velocity P"
        rows={[row(0), row(1, { best: true, gap: null }), row(2, { gap: 5.1, tolerance: 3.1 })]}
        speeds={[100, 400]}
        {...over}
      />
    </TooltipProvider>
  );
}

test("the columns use words a person would: gain, status, score, behind best, damping and the error at each speed", () => {
  render(table());
  const names = within(screen.getByRole("table", { name: "Velocity P results" }))
    .getAllByRole("columnheader")
    .map((c) => c.textContent);
  expect(names).toEqual([
    "#",
    "Gain",
    "Status",
    "Score (μm)",
    "Behind best",
    "Damping",
    "Error at 100 mm/s (μm)",
    "Error at 400 mm/s (μm)",
  ]);
});

test("the headers are not forced to upper case, which turns μm into a capital M", () => {
  render(table());
  const head = screen.getByRole("table", { name: "Velocity P results" }).querySelector("thead") as HTMLElement;
  expect(head.className).not.toMatch(/uppercase/);
});

test("the gain is the written value, once", () => {
  render(table());
  const first = within(screen.getByRole("table", { name: "Velocity P results" })).getAllByRole("row")[1];
  expect(within(first).getAllByRole("cell")[1]).toHaveTextContent("841");
  expect(within(first).getAllByRole("cell")).toHaveLength(8);
});

test("a corner ladder shows its gains in Hz", () => {
  render(
    table({ axis: "VelocityICornerHz", label: "Velocity I corner (Hz)", rows: [row(0, { shown: 50.04, raw: 4200 })] }),
  );
  const first = within(screen.getByRole("table", { name: "Velocity I corner (Hz) results" })).getAllByRole("row")[1];
  expect(within(first).getAllByRole("cell")[1]).toHaveTextContent("50.0 Hz");
});

test("the score, damping and error headers explain themselves", async () => {
  render(table());
  fireEvent.focus(screen.getByText("Score (μm)"));
  expect(await screen.findByRole("tooltip")).toHaveTextContent(
    "The worst tracking error across your test speeds. Lower is better.",
  );
});

test("damping and error headers have their own explanations", async () => {
  const { unmount } = render(table());
  fireEvent.focus(screen.getByText("Damping"));
  expect(await screen.findByRole("tooltip")).toHaveTextContent(/steadier/);
  unmount();
  render(table());
  fireEvent.focus(screen.getByText("Error at 100 mm/s (μm)"));
  expect(await screen.findByRole("tooltip")).toHaveTextContent(/99% of the time/);
});

test("the best gain says so and the others say how far behind", () => {
  render(table());
  const rows = within(screen.getByRole("table", { name: "Velocity P results" })).getAllByRole("row");
  expect(within(rows[2]).getByText("best")).toBeInTheDocument();
  expect(within(rows[3]).getByText("5.1 μm ± 3.1 μm")).toBeInTheDocument();
});

test("each row is numbered from 1, in the order the gains are run", () => {
  render(table());
  const rows = within(screen.getByRole("table", { name: "Velocity P results" }))
    .getAllByRole("row")
    .slice(1);
  expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["1", "2", "3"]);
});

test("a status with detail explains itself on hover", async () => {
  render(
    table({
      rows: [
        row(0, {
          verdict: "Skipped: damping too low",
          detail: "Median damping 0.011 was below the stop limit of 0.02.",
        }),
      ],
    }),
  );
  fireEvent.focus(screen.getByText("Skipped: damping too low"));
  expect(await screen.findByRole("tooltip")).toHaveTextContent(
    "Median damping 0.011 was below the stop limit of 0.02.",
  );
});

test("the recommended gain is not followed by a technical suffix", () => {
  render(table({ rows: [row(0, { verdict: "Recommended", recommended: "measured" })] }));
  expect(screen.queryByText("(measured)")).toBeNull();
});
