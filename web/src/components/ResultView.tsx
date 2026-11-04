import { useState } from "react";
import type { TuneDetail } from "@/api";
import { gainChecklist, resultRows } from "@/guided";
import { GainChecklist } from "./ResultPlaceholder";
import { Button, Card } from "./ui";

/** The finished tune: what it changed, and the config block to keep the new gains. */
export function ResultView({ tune, block }: { tune: TuneDetail; block: string }) {
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
              </tr>
            </thead>
            <tbody className="font-mono">
              {resultRows(tune).map((r) => (
                <tr key={r.label}>
                  <td className="pr-6 font-sans">{r.label}</td>
                  <td className="pr-6">{r.before}</td>
                  <td className="pr-6">{r.after}</td>
                  <td className="pr-6 font-sans text-muted-foreground">{r.change}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Card>
      <Card label="Printer config" action={<Button onClick={copy}>{copied ? "Copied" : "Copy"}</Button>}>
        <p className="mb-3 text-sm">
          The stepper is using these gains now. To keep them after a restart, replace the values in the [foci{" "}
          {tune.setup.stepper}] section of your printer config with this block.
        </p>
        <pre className="overflow-x-auto font-mono text-xs">{block}</pre>
      </Card>
    </>
  );
}
