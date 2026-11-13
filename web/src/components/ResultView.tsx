import { CheckCircle2Icon, CheckIcon, CopyIcon } from "lucide-react";
import { Button as UiButton } from "@/components/ui/button";
import { useState } from "react";
import type { TuneAxis, TuneDetail } from "@/api";
import { changedGains, gainChecklist, resultRows, type LiveControl } from "@/guided";
import { Explained } from "./Explained";
import { GainChecklist } from "./ResultPlaceholder";
import { Card } from "./ui";

const BLOCK_KEY_AXIS: Record<string, TuneAxis> = {
  pid_velocity_p: "VelocityP",
  pid_velocity_i: "VelocityICornerHz",
  pid_position_p: "PositionP",
};

export function ResultView({ tune, block, live }: { tune: TuneDetail; block: string; live: LiveControl | null }) {
  const [copied, setCopied] = useState(false);
  const changed = changedGains(tune);
  const copy = () => {
    void navigator.clipboard.writeText(block).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  };
  return (
    <>
      <Card label="Result">
        <GainChecklist items={gainChecklist(tune)} />
        <div className="mt-4 overflow-x-auto">
          <table aria-label="Before and after" className="w-full text-left text-sm">
            <thead className="text-xs text-muted-foreground">
              <tr>
                <th className="pr-6 align-top font-medium">Gain</th>
                <th className="pr-6 align-top font-medium">Before</th>
                <th className="pr-6 align-top font-medium">After</th>
                <th className="pr-6 align-top font-medium">Change</th>
                <th className="pr-6 align-top font-medium">
                  <Explained tip="The stepper is running this gain right now.">Active</Explained>
                </th>
              </tr>
            </thead>
            <tbody className="font-mono">
              {resultRows(tune, live).map((r) => (
                <tr key={r.label}>
                  <td className="pr-6 font-sans">{r.label}</td>
                  <td className="pr-6">{r.before}</td>
                  <td className="pr-6">{r.after}</td>
                  <td className="pr-6 font-sans text-muted-foreground">{r.change}</td>
                  <td className="pr-6">
                    {r.active && <CheckCircle2Icon aria-hidden="true" className="size-4 text-emerald-400" />}
                    <span className="sr-only">{r.active ? "Active" : "Not active"}</span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Card>
      <Card
        label="Printer config"
        action={
          <UiButton variant="outline" size="icon" aria-label={copied ? "Copied" : "Copy"} onClick={copy}>
            {copied ? <CheckIcon /> : <CopyIcon />}
          </UiButton>
        }
      >
        <pre className="overflow-x-auto font-mono text-xs">
          {(block.match(/[^\n]*\n|[^\n]+$/g) ?? []).map((line, i) => {
            const axis = BLOCK_KEY_AXIS[line.split(":")[0]];
            return (
              // biome-ignore lint/suspicious/noArrayIndexKey: the lines of a fixed block never reorder.
              <span key={i} className={axis !== undefined && changed[axis] ? "text-emerald-400" : undefined}>
                {line}
              </span>
            );
          })}
        </pre>
      </Card>
    </>
  );
}
