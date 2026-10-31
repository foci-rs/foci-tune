import { createRootRoute, Outlet } from "@tanstack/react-router";

export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  return (
    <div className="min-h-screen">
      <header className="border-b px-4 py-3 text-sm font-medium tracking-wide">
        foci-tune
      </header>
      <Outlet />
    </div>
  );
}
