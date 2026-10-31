import { RouterProvider, createMemoryHistory, createRouter } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { Route as rootRoute } from "@/routes/__root";

test("renders the header placeholder", async () => {
  const router = createRouter({
    routeTree: rootRoute,
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(<RouterProvider router={router} />);
  expect(await screen.findByRole("banner")).toHaveTextContent("foci-tune");
});
