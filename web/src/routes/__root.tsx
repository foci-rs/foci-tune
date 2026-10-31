import { createRootRoute, Outlet, redirect } from "@tanstack/react-router";
import { Header } from "@/components/header";
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
      <div className="min-h-screen">
        <Header />
        <Outlet />
      </div>
    </FeedProvider>
  );
}
