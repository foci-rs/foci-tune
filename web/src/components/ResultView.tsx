import { CheckCircle2Icon } from "lucide-react";
import { useState } from "react";
import type { TuneDetail } from "@/api";
import { gainChecklist, resultRows, type LiveControl } from "@/guided";
import { Explained } from "./Explained";
import { GainChecklist } from "./ResultPlaceholder";
import { Button, Card } from "./ui";

/** The finished tune: what it changed, and the config block to keep the new gains. */
export function ResultView({ tune, block, live }: { tune: TuneDetail; block: string; live: LiveControl | null }) {
  const [copied, setCopied] = useState(false);
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
      <Card label="Printer config" action={<Button onClick={copy}>{copied ? "Copied" : "Copy"}</Button>}>
        <p className="mb-3 text-sm">
          To keep them after a restart, replace the [foci {tune.setup.stepper}] section of your printer config with
          this.
        </p>
        <pre className="overflow-x-auto font-mono text-xs">{block}</pre>
      </Card>
    </>
  );
}
