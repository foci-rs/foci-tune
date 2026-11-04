import { render, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import type { CurrentRunView } from "@/api";
import { RunQueueCard } from "./RunQueueCard";

const current = (state: string, extra: object = {}) =>
  ({ session: "20261104-aaa", run: "20261104T042243Z-qtk7", state, ...extra }) as CurrentRunView;

const show = (over: Partial<Parameters<typeof RunQueueCard>[0]> = {}) =>
  render(
    <RunQueueCard
      current={current("verifying")}
      otherSession={false}
      position={null}
      steps={[]}
      predictedS={null}
      nowMs={0}
      pending={[]}
      queued={0}
      {...over}
    />,
  );

test("the running run has its own rows: run, status and progress", () => {
  show();
  const card = screen.getByRole("region", { name: "Run queue" });
  expect(within(card).getByText("Run")).toBeInTheDocument();
  expect(card).toHaveTextContent("20261104T042243Z-qtk7");
  expect(within(card).getByText("Status")).toBeInTheDocument();
  expect(card).toHaveTextContent("Verifying");
  const bar = within(card).getByRole("progressbar", { name: "Run progress" });
  expect(bar).toHaveAttribute("aria-valuenow", "93");
  expect(bar).toHaveAttribute("aria-valuemax", "100");
  expect(card).toHaveTextContent("Step 7 of 8");
});

test("the bar starts empty and the motion shows the time left", () => {
  const { rerender } = show({ current: current("preparing") });
  expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "0");
  rerender(
    <RunQueueCard
      current={current("moving")}
      otherSession={false}
      position={null}
      steps={[{ step: "moving", unix_ms: 10_000 }]}
      predictedS={40}
      nowMs={30_000}
      pending={[]}
      queued={0}
    />,
  );
  expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "50");
  expect(screen.getByRole("region", { name: "Run queue" })).toHaveTextContent("Step 5 of 8 · about 20 s left");
});

test("a sweep shows which run this is, and a reason sits beside the status", () => {
  show({ position: { run: 3, of: 12 }, current: current("moving", { reason: "stroke 2" }) });
  const card = screen.getByRole("region", { name: "Run queue" });
  expect(card).toHaveTextContent("Run 3 of 12");
  expect(card).toHaveTextContent("stroke 2");
});

test("queued runs are counted and listed in the order they will run", () => {
  show({
    queued: 3,
    pending: [
      { id: "a", state: "queued", value: 100 },
      { id: "b", state: "queued", value: 120 },
      { id: "c", state: "queued", value: null },
    ],
  });
  const card = screen.getByRole("region", { name: "Run queue" });
  expect(card).toHaveTextContent("3 queued");
  const items = within(screen.getByRole("list", { name: "Queue" })).getAllByRole("listitem");
  expect(items.map((i) => i.textContent)).toEqual(["100 · queued", "120 · queued", "queued"]);
});

test("with nothing running only the queue shows", () => {
  show({ current: null, queued: 2 });
  const card = screen.getByRole("region", { name: "Run queue" });
  expect(card).toHaveTextContent("2 queued");
  expect(within(card).queryByRole("progressbar")).toBeNull();
  expect(within(card).queryByText("Status")).toBeNull();
});

test("a sweep row shows the whole step's progress next to the run's, counting the run in flight", () => {
  show({ current: current("preparing"), sweep: { done: 10, total: 100 } });
  const card = screen.getByRole("region", { name: "Run queue" });
  const bar = within(card).getByRole("progressbar", { name: "Sweep progress" });
  expect(bar).toHaveAttribute("aria-valuenow", "10");
  expect(card).toHaveTextContent("10 of 100 runs");
  expect(within(card).getByRole("progressbar", { name: "Run progress" })).toBeInTheDocument();
});

test("between two runs the sweep row is still there", () => {
  show({ current: null, sweep: { done: 10, total: 100 }, showQueue: false });
  expect(screen.getByRole("progressbar", { name: "Sweep progress" })).toHaveAttribute("aria-valuenow", "10");
  expect(screen.queryByText("Queue")).toBeNull();
});

test("a standstill run reads Measuring in its motion stage, a probe run Moving", () => {
  const { rerender } = show({ current: current("moving"), kind: "standstill" });
  expect(screen.getByRole("region", { name: "Run queue" })).toHaveTextContent("Measuring");
  rerender(
    <RunQueueCard
      current={current("moving")}
      kind="probe"
      otherSession={false}
      position={null}
      steps={[]}
      predictedS={null}
      nowMs={0}
      pending={[]}
      queued={0}
    />,
  );
  expect(screen.getByRole("region", { name: "Run queue" })).toHaveTextContent("Moving");
});
