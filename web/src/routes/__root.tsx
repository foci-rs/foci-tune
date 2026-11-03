import { createRootRoute, Outlet, redirect } from "@tanstack/react-router";
import { AppSidebar } from "@/components/app-sidebar";
import { Header } from "@/components/header";
import { SidebarProvider } from "@/components/ui/sidebar";
import { FeedProvider } from "@/feed";

export const Route = createRootRoute({
  beforeLoad: ({ location }) => {
    if (location.pathname === "/") throw redirect({ to: "/printer" });
  },
  component: RootLayout,
});

function RootLayout() {
  return (
    <FeedProvider>
      <SidebarProvider className="min-h-screen flex-col [--header-height:--spacing(14)]">
        <Header />
        <div className="flex flex-1">
          <AppSidebar />
          <div className="min-w-0 flex-1">
            <Outlet />
          </div>
        </div>
      </SidebarProvider>
    </FeedProvider>
  );
}
