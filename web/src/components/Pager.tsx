import { Link } from "@tanstack/react-router";
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from "@/components/ui/pagination";
import { type Page, pageLinks } from "@/sessions";

export type PageParam = "page" | "tunes";

/** The range shown and numbered links to the other pages; renders nothing for a single page. Each link changes only `param`, so a page kept in the address by another list stays. */
export function Pager({ page, param, label }: { page: Page<unknown>; param: PageParam; label: string }) {
  if (page.pages <= 1) return null;
  const to = (n: number) => (prev: { page?: number; tunes?: number }) => ({ ...prev, [param]: n > 1 ? n : undefined });
  return (
    <div className="mt-3 flex items-center justify-between gap-4 text-sm text-muted-foreground">
      <span>
        {page.from}–{page.to} of {page.total}
      </span>
      <Pagination aria-label={label} className="mx-0 w-auto">
        <PaginationContent>
          {page.page > 1 && (
            <PaginationItem>
              <PaginationPrevious asChild>
                <Link to="/sessions" search={to(page.page - 1)} />
              </PaginationPrevious>
            </PaginationItem>
          )}
          {pageLinks(page.page, page.pages).map((n, i) =>
            n === "gap" ? (
              // biome-ignore lint/suspicious/noArrayIndexKey: the gaps have no identity of their own and never reorder.
              <PaginationItem key={`gap-${i}`}>
                <PaginationEllipsis />
              </PaginationItem>
            ) : (
              <PaginationItem key={n}>
                <PaginationLink asChild isActive={n === page.page}>
                  <Link to="/sessions" search={to(n)}>
                    {n}
                  </Link>
                </PaginationLink>
              </PaginationItem>
            ),
          )}
          {page.page < page.pages && (
            <PaginationItem>
              <PaginationNext asChild>
                <Link to="/sessions" search={to(page.page + 1)} />
              </PaginationNext>
            </PaginationItem>
          )}
        </PaginationContent>
      </Pagination>
    </div>
  );
}
