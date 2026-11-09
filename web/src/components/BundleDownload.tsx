import { DownloadIcon } from "lucide-react";

/** Icon link that downloads a support bundle; `what` names it for screen readers and the tooltip. */
export function BundleDownload({ href, what }: { href: string; what: string }) {
  const label = `Download ${what} for support`;
  return (
    <a
      href={href}
      download
      aria-label={label}
      title={label}
      className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
    >
      <DownloadIcon className="size-4" />
    </a>
  );
}
