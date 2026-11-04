import type { RestRecord } from "@/api";
import { formatNumber } from "@/tune";
import type { OverlayRun } from "./TracePanels";

const HEIGHT = 160;
const BAR = 10;
const GAP = 2;
const REST_ROWS = [
  { label: "torque sd", key: "torque_sd" },
  { label: "transitions /s", key: "transitions_per_s" },
  { label: "large steps /s", key: "large_transitions_per_s" },
] as const;

/** Torque standard deviation per rest, one bar per checked run in each rest group. */
export function RestBars({ runs }: { runs: OverlayRun<RestRecord[]>[] }) {
  const rests = Math.max(0, ...runs.map((r) => r.value.length));
  const max = Math.max(0, ...runs.flatMap((r) => r.value.map((v) => v.torque_sd ?? 0)));
  if (rests === 0) return <p className="text-sm text-muted-foreground">No rests measured.</p>;
  const group = runs.length * (BAR + GAP) + 12;
  const width = rests * group + 40;
  return (
    <div className="overflow-x-auto">
      <svg
        role="img"
        aria-label="Torque sd per rest"
        width={width}
        height={HEIGHT + 24}
        className="text-muted-foreground"
      >
        <line x1={32} x2={width} y1={HEIGHT} y2={HEIGHT} stroke="currentColor" strokeOpacity={0.3} />
        <text x={0} y={10} fontSize={10} fill="currentColor">
          {formatNumber(max, 1)}
        </text>
        {Array.from({ length: rests }, (_, k) => (
          <g key={k} transform={`translate(${36 + k * group},0)`}>
            {runs.map((r, j) => {
              const sd = r.value[k]?.torque_sd ?? null;
              const h = sd === null || max === 0 ? 0 : (sd / max) * (HEIGHT - 14);
              return (
                <rect key={r.id} x={j * (BAR + GAP)} y={HEIGHT - h} width={BAR} height={h} rx={2} fill={r.color}>
                  <title>{`${r.label}, rest ${k + 1}: torque sd ${formatNumber(sd, 1)}`}</title>
                </rect>
              );
            })}
            <text x={0} y={HEIGHT + 14} fontSize={10} fill="currentColor">
              {k + 1}
            </text>
          </g>
        ))}
      </svg>
      <table className="mt-2 text-xs">
        <caption className="sr-only">Torque sd per rest</caption>
        <thead className="text-muted-foreground">
          <tr>
            <th className="pr-3 text-left font-medium">Run</th>
            <th className="pr-3 text-left font-medium">Metric</th>
            {Array.from({ length: rests }, (_, k) => (
              <th key={k} className="pr-3 text-right font-medium">
                {k + 1}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="font-mono">
          {runs.flatMap((r) =>
            REST_ROWS.map((row, i) => (
              <tr key={`${r.id}-${row.label}`}>
                {i === 0 && (
                  <td className="pr-3" rowSpan={REST_ROWS.length}>
                    <span
                      aria-hidden="true"
                      className="mr-1 inline-block size-2 rounded-sm"
                      style={{ background: r.color }}
                    />
                    {r.label}
                  </td>
                )}
                <th scope="row" className="pr-3 text-left font-normal text-muted-foreground">
                  {row.label}
                </th>
                {Array.from({ length: rests }, (_, k) => (
                  <td key={k} className="pr-3 text-right">
                    {formatNumber(r.value[k]?.[row.key] ?? null, 1)}
                  </td>
                ))}
              </tr>
            )),
          )}
        </tbody>
      </table>
    </div>
  );
}
