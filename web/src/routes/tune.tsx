import { createRoute } from "@tanstack/react-router";
import { EmptyState } from "@/components/ui";
import { Route as rootRoute } from "./__root";

export const Route = createRoute({
  getParentRoute: () => rootRoute,
  path: "/tune",
  component: () => <EmptyState heading="No session open" />,
});
