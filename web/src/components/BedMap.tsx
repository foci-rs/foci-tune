import { useRef, useState, type PointerEvent } from "react";
import type { Pattern } from "@/api";
import { clampPoint, PATTERN_MARGIN_MM, type Limits } from "@/tune";
import { NumberField } from "./ui";

type End = "start" | "end";

const round = (v: number) => Math.round(v * 10) / 10;

function CoordField({
  label,
  value,
  disabled,
  onValue,
}: {
  label: string;
  value: number;
  disabled: boolean;
  onValue: (v: number) => void;
}) {
  return (
    <label className="flex items-center gap-1 text-xs text-muted-foreground">
      {label}
      <NumberField label={label} value={value} step={0.1} disabled={disabled} onValue={onValue} className="w-20" />
    </label>
  );
}

/** The bed seen from above, with the stroke pattern's endpoints draggable inside the margin. */
export function BedMap({
  limits,
  pattern,
  proposal,
  disabled = false,
  onChange,
}: {
  limits: Limits;
  pattern: Pattern;
  proposal: Pattern | null;
  disabled?: boolean;
  onChange: (pattern: Pattern) => void;
}) {
  const svg = useRef<SVGSVGElement>(null);
  const [dragging, setDragging] = useState<End | null>(null);
  const [x0, x1] = limits.x;
  const [y0, y1] = limits.y;
  const w = x1 - x0;
  const h = y1 - y0;
  const r = Math.max(w, h) / 60;
  const toSvg = ([x, y]: [number, number]) => [x - x0, y1 - y] as const;

  const bedPoint = (e: PointerEvent): [number, number] | null => {
    const el = svg.current;
    const ctm = el?.getScreenCTM();
    if (!el || !ctm) return null;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(ctm.inverse());
    return clampPoint([round(p.x + x0), round(y1 - p.y)], limits);
  };

  const move = (e: PointerEvent) => {
    if (dragging === null) return;
    const p = bedPoint(e);
    if (p !== null) onChange({ ...pattern, [dragging]: p });
  };

  const handle = (end: End) => {
    const [cx, cy] = toSvg(pattern[end]);
    return (
      <circle
        cx={cx}
        cy={cy}
        r={r}
        aria-label={`${end} point`}
        className={
          end === "start"
            ? "cursor-grab fill-emerald-400 stroke-background"
            : "cursor-grab fill-sky-400 stroke-background"
        }
        strokeWidth={r / 3}
        onPointerDown={(e) => {
          if (disabled) return;
          (e.target as Element).setPointerCapture(e.pointerId);
          setDragging(end);
        }}
      />
    );
  };

  const [ps, pe] = [toSvg(pattern.start), toSvg(pattern.end)];
  const set = (end: End, k: 0 | 1, v: number) => {
    const point: [number, number] = [...pattern[end]];
    point[k] = v;
    onChange({ ...pattern, [end]: point });
  };

  return (
    <div className="space-y-2">
      <svg
        ref={svg}
        role="img"
        aria-label={`Bed map, ${w} by ${h} mm, pattern from ${pattern.start.join(", ")} to ${pattern.end.join(", ")}`}
        viewBox={`${-r * 2} ${-r * 2} ${w + r * 4} ${h + r * 4}`}
        className="h-64 w-full touch-none select-none"
        onPointerMove={move}
        onPointerUp={() => setDragging(null)}
        onPointerCancel={() => setDragging(null)}
      >
        <rect x={0} y={0} width={w} height={h} className="fill-muted stroke-border" strokeWidth={r / 4} />
        <rect
          x={PATTERN_MARGIN_MM}
          y={PATTERN_MARGIN_MM}
          width={Math.max(0, w - 2 * PATTERN_MARGIN_MM)}
          height={Math.max(0, h - 2 * PATTERN_MARGIN_MM)}
          fill="none"
          className="stroke-muted-foreground/40"
          strokeWidth={r / 5}
          strokeDasharray={`${r} ${r}`}
        />
        {proposal !== null && (
          <line
            x1={toSvg(proposal.start)[0]}
            y1={toSvg(proposal.start)[1]}
            x2={toSvg(proposal.end)[0]}
            y2={toSvg(proposal.end)[1]}
            className="stroke-muted-foreground"
            strokeWidth={r / 3}
            strokeDasharray={`${r} ${r / 2}`}
          >
            <title>Proposed pattern</title>
          </line>
        )}
        <line x1={ps[0]} y1={ps[1]} x2={pe[0]} y2={pe[1]} className="stroke-emerald-400" strokeWidth={r / 2} />
        {handle("start")}
        {handle("end")}
      </svg>
      <div className="flex flex-wrap gap-x-4 gap-y-1">
        <CoordField label="Start X" value={pattern.start[0]} disabled={disabled} onValue={(v) => set("start", 0, v)} />
        <CoordField label="Start Y" value={pattern.start[1]} disabled={disabled} onValue={(v) => set("start", 1, v)} />
        <CoordField label="End X" value={pattern.end[0]} disabled={disabled} onValue={(v) => set("end", 0, v)} />
        <CoordField label="End Y" value={pattern.end[1]} disabled={disabled} onValue={(v) => set("end", 1, v)} />
      </div>
      <p className="text-xs text-muted-foreground">
        Start is green, end is blue; the dashed line is the proposed pattern and the dashed box the{" "}
        {PATTERN_MARGIN_MM} mm margin.
      </p>
    </div>
  );
}
