import { Link } from "@tanstack/react-router";
import { ActivityIcon, HistoryIcon, PrinterIcon, type LucideIcon } from "lucide-react";
import { useFeed } from "@/feed";
import { formatBytes } from "@/units";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";

const pages: { to: "/printer" | "/tune" | "/sessions"; label: string; icon: LucideIcon }[] = [
  { to: "/printer", label: "Printer", icon: PrinterIcon },
  { to: "/tune", label: "Tune", icon: ActivityIcon },
  { to: "/sessions", label: "Sessions", icon: HistoryIcon },
];

export function AppSidebar() {
  return (
    <Sidebar
      collapsible="none"
      className="sticky top-(--header-height) h-[calc(100svh-var(--header-height))] shrink-0 border-r"
    >
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" asChild>
              <Link to="/printer">
                <img src="/foci-icon.svg" alt="" className="size-8 rounded-lg" />
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-medium">FOCI</span>
                  <span className="truncate text-xs text-muted-foreground">Motion Tuning</span>
                </div>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <nav aria-label="Pages">
            <SidebarMenu>
              {pages.map(({ to, label, icon: Icon }) => (
                <SidebarMenuItem key={to}>
                  <SidebarMenuButton asChild>
                    <Link to={to} activeProps={{ "data-active": true }}>
                      <Icon />
                      <span>{label}</span>
                    </Link>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </nav>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter>
        <PrinterStatus />
      </SidebarFooter>
    </Sidebar>
  );
}

function PrinterStatus() {
  const { printer } = useFeed();
  return (
    <section aria-label="Printer status" className="space-y-2 p-2 text-sm">
      <ul className="space-y-1">
        <li className="font-mono text-muted-foreground">
          Homed {printer && printer.moonraker.homed_axes !== "" ? printer.moonraker.homed_axes : "none"}
        </li>
        <li className="font-mono text-muted-foreground">
          Free {printer?.free_bytes != null ? formatBytes(printer.free_bytes) : "unknown"}
        </li>
      </ul>
    </section>
  );
}
