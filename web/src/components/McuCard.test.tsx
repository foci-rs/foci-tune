import { render, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import type { BoardView, PrinterView, StepperView } from "@/api";
import { mcuName } from "@/mcu";
import { McuCard } from "./McuCard";

const board = (over: Partial<BoardView> = {}): BoardView =>
  ({
    serial: "0029002A",
    open: true,
    protocol_version: 25,
    unusable_reason: null,
    trace_mode: "streaming",
    critical_hz_measured: 49987.4,
    firmware_id: "ouroboros-fw",
    product: "Ouroboros",
    mcu_version: "v0.4.1-30-g4f2b042b",
    mcu_build: "fw=68e16122",
    channels: [{ channel: 0 }, { channel: 1 }],
    ...over,
  }) as BoardView;

const printer = (steppers: Partial<StepperView>[] = []) => ({ steppers }) as PrinterView;

test("the card shows the board's identity, firmware, trace state, cadence and channels", () => {
  render(<McuCard printer={printer([{ serial: "0029002A", mcu: "ouroboros" }])} board={board()} />);
  const card = screen.getByRole("region", { name: "Ouroboros (2 Channels)" });
  expect(card).toHaveTextContent("Chip ID0029002A");
  expect(card).toHaveTextContent("BoardOuroboros");
  expect(card).toHaveTextContent("Firmwarev0.4.1-30-g4f2b042b");
  expect(within(card).getByText("v0.4.1-30-g4f2b042b")).toHaveAttribute("title", "fw=68e16122");
  expect(card).not.toHaveTextContent("Protocol");
  expect(card).toHaveTextContent("TraceStreaming");
  expect(card).toHaveTextContent("Sampling frequency49987 Hz");
  expect(card).not.toHaveTextContent("Open");
});

test("a board that is not streaming shows no cadence, and a closed board says so", () => {
  render(
    <McuCard printer={printer()} board={board({ open: false, trace_mode: "dormant", critical_hz_measured: 0 })} />,
  );
  const card = screen.getByRole("region", { name: "0029002A (2 Channels)" });
  expect(card).toHaveTextContent("TraceOff");
  expect(card).toHaveTextContent("Sampling frequency-");
  expect(card).toHaveTextContent("Closed");
});

test("a single channel is not pluralised", () => {
  render(<McuCard printer={printer()} board={board({ channels: [{ channel: 0 }] as never })} />);
  expect(screen.getByRole("region", { name: "0029002A (1 Channel)" })).toBeInTheDocument();
});

test("mcuName title-cases the Klipper name and falls back to the serial", () => {
  const p = printer([
    { serial: "AAA", mcu: "tool_head" },
    { serial: "BBB", mcu: "ouroboros" },
  ]);
  expect(mcuName(p, board({ serial: "AAA" }))).toBe("Tool Head");
  expect(mcuName(p, board({ serial: "BBB" }))).toBe("Ouroboros");
  expect(mcuName(p, board({ serial: "CCC" }))).toBe("CCC");
});
