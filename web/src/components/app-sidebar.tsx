import { Link } from "@tanstack/react-router";
import {
  FolderClockIcon,
  PrinterIcon,
  SlidersHorizontalIcon,
  SlidersVerticalIcon,
  WandSparklesIcon,
  type LucideIcon,
} from "lucide-react";
import { useFeed } from "@/feed";
import { formatBytes } from "@/units";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";

const pages: { to: "/printer" | "/tune" | "/sessions"; label: string; icon: LucideIcon }[] = [
  { to: "/printer", label: "Printer", icon: PrinterIcon },
  { to: "/tune", label: "Tune", icon: SlidersHorizontalIcon },
  { to: "/sessions", label: "Sessions", icon: FolderClockIcon },
];

const tuneModes: { to: "/tune/manual" | "/tune/guided"; label: string; icon: LucideIcon }[] = [
  { to: "/tune/manual", label: "Manual", icon: SlidersVerticalIcon },
  { to: "/tune/guided", label: "Guided", icon: WandSparklesIcon },
];

export function AppSidebar() {
  return (
    <Sidebar
      collapsible="none"
      className="sticky top-(--header-height) h-[calc(100svh-var(--header-height))] shrink-0 border-r"
    >
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
                  {to === "/tune" && (
                    <SidebarMenuSub aria-label="Tune modes">
                      {tuneModes.map((mode) => (
                        <SidebarMenuSubItem key={mode.to}>
                          <SidebarMenuSubButton asChild>
                            <Link to={mode.to} activeProps={{ "data-active": true }}>
                              <mode.icon />
                              <span>{mode.label}</span>
                            </Link>
                          </SidebarMenuSubButton>
                        </SidebarMenuSubItem>
                      ))}
                    </SidebarMenuSub>
                  )}
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
          Disk Free {printer?.free_bytes != null ? formatBytes(printer.free_bytes) : "unknown"}
        </li>
        <li className="font-mono text-muted-foreground">Version {printer?.version ?? "unknown"}</li>
      </ul>
    </section>
  );
}
