import { useRef, useState, type PointerEvent, type ReactNode } from "react";
import type { Pattern } from "@/api";
import { clampPoint, PATTERN_MARGIN_MM, type Limits } from "@/tune";
import { NumberField } from "./ui";

type End = "start" | "end";

const round = (v: number) => Math.round(v);

function CoordField({
  label,
  axis,
  value,
  disabled,
  onValue,
}: {
  label: string;
  axis: string;
  value: number;
  disabled: boolean;
  onValue: (v: number) => void;
}) {
  return (
    <label className="flex items-center gap-1 text-xs text-muted-foreground">
      {axis}
      <NumberField
        label={label}
        value={Math.round(value)}
        step={1}
        disabled={disabled}
        onValue={(v) => onValue(Math.round(v))}
        className="w-24"
      />
    </label>
  );
}

function CoordRow({
  end,
  label,
  point,
  disabled,
  onValue,
}: {
  end: End;
  label: string;
  point: [number, number];
  disabled: boolean;
  onValue: (k: 0 | 1, v: number) => void;
}) {
  return (
    <div role="group" aria-label={label} className="flex items-center gap-3">
      <span className="flex w-14 items-center gap-1.5 text-xs text-muted-foreground">
        <span
          aria-hidden="true"
          className={end === "start" ? "size-2 rounded-full bg-emerald-400" : "size-2 rounded-full bg-sky-400"}
        />
        {label}
      </span>
      <CoordField label={`${label} X`} axis="X" value={point[0]} disabled={disabled} onValue={(v) => onValue(0, v)} />
      <CoordField label={`${label} Y`} axis="Y" value={point[1]} disabled={disabled} onValue={(v) => onValue(1, v)} />
    </div>
  );
}

/** The bed seen from above, with the stroke pattern's endpoints draggable inside the margin. */
export function BedMap({
  limits,
  pattern,
  proposal,
  proposalTitle = "Proposed pattern",
  disabled = false,
  onChange,
  children,
}: {
  limits: Limits;
  pattern: Pattern;
  proposal: Pattern | null;
  /** Hover title of the proposal line. */
  proposalTitle?: string;
  disabled?: boolean;
  onChange: (pattern: Pattern) => void;
  /** Shown between the map and the coordinate rows. */
  children?: ReactNode;
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
        >
          <title>{PATTERN_MARGIN_MM} mm margin</title>
        </rect>
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
            <title>{proposalTitle}</title>
          </line>
        )}
        <line x1={ps[0]} y1={ps[1]} x2={pe[0]} y2={pe[1]} className="stroke-emerald-400" strokeWidth={r / 2} />
        {handle("start")}
        {handle("end")}
      </svg>
      <p className="text-center text-xs text-muted-foreground">
        Length{" "}
        <span className="font-mono text-foreground">
          {Math.round(Math.hypot(pattern.end[0] - pattern.start[0], pattern.end[1] - pattern.start[1]))} mm
        </span>
      </p>
      {children}
      <div className="space-y-2">
        <CoordRow
          end="start"
          label="Start"
          point={pattern.start}
          disabled={disabled}
          onValue={(k, v) => set("start", k, v)}
        />
        <CoordRow end="end" label="End" point={pattern.end} disabled={disabled} onValue={(k, v) => set("end", k, v)} />
      </div>
    </div>
  );
}
