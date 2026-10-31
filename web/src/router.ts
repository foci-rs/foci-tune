import { createMemoryHistory, createRouter, type RouterHistory } from "@tanstack/react-router";
import { Route as rootRoute } from "@/routes/__root";
import { Route as printerRoute } from "@/routes/printer";
import { Route as sessionRoute } from "@/routes/sessions.$id";
import { Route as sessionsRoute } from "@/routes/sessions";
import { Route as tuneRoute } from "@/routes/tune";

const routeTree = rootRoute.addChildren([printerRoute, tuneRoute, sessionsRoute, sessionRoute]);

export function createAppRouter(history?: RouterHistory) {
  return createRouter({ routeTree, history });
}

export function createTestRouter(path: string) {
  return createAppRouter(createMemoryHistory({ initialEntries: [path] }));
}

export const router = createAppRouter();

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
