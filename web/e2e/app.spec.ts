import { test, expect } from "@playwright/test";

// Smoke test: the landing/join UI renders with no console errors or
// unhandled promise rejections. This is the cheapest possible real
// regression — it would have caught, e.g., a top-level import throwing.
test.describe("app shell", () => {
  test("loads and renders the join UI without console errors", async ({ page }) => {
    const consoleErrors: string[] = [];
    const pageErrors: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });
    page.on("pageerror", (err) => pageErrors.push(String(err)));

    await page.goto("/");

    await expect(page.locator("h1")).toHaveText("couchlink");
    await expect(
      page.getByPlaceholder("paste the link your host sent — or session:pin")
    ).toBeVisible();
    await expect(page.getByPlaceholder("friends-night")).toBeVisible();
    await expect(page.getByPlaceholder("6 digits")).toBeVisible();
    await expect(page.getByRole("button", { name: "Join session" })).toBeVisible();

    expect(consoleErrors, `console errors: ${consoleErrors.join("\n")}`).toEqual([]);
    expect(pageErrors, `page errors: ${pageErrors.join("\n")}`).toEqual([]);
  });

  test("basic layout: roster and top pill render", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("ol.roster")).toBeVisible();
    await expect(page.locator("li.roster-slot")).toHaveCount(4);
    await expect(page.locator(".pill")).toBeVisible();
  });

  test("DebugDrawer/stats panel toggles open without crashing", async ({ page }) => {
    const pageErrors: string[] = [];
    page.on("pageerror", (err) => pageErrors.push(String(err)));

    await page.goto("/");
    const toggle = page.locator(".dt-toggle");
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");

    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");

    // Closing it again should also not throw.
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");

    expect(pageErrors, `page errors: ${pageErrors.join("\n")}`).toEqual([]);
  });

  // A real join requires a live signaling server + host peer on the other
  // end of the WebRTC connection, neither of which exists in this headless
  // suite. Exercising `player.connect()` end to end (ICE, DataChannel open,
  // first video frame) needs an actual host process and is left to manual /
  // integration testing against a running couchlink host.
  test.skip("full join flow against a live host peer — needs a running signaling server + host", () => {});
});
