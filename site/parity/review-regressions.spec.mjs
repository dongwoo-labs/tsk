import { test, expect } from "@playwright/test";

const LANDING = "http://127.0.0.1:4180/";

test("board illustration uses an arrow without a filled selection or a glyph", async ({
  page,
}) => {
  await page.goto(LANDING);
  const row = page.locator(".screen .s-task.is-sel");
  await expect(row).toContainText("▸  T31");
  await expect(row).not.toContainText("●");
  await expect(row).toHaveCSS("background-image", "none");
  await expect(row).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(row.locator(".sel-text")).toHaveCSS(
    "background-color",
    "rgba(0, 0, 0, 0)",
  );
});

test("the done drawer and its archived group follow the scope", async ({
  page,
}) => {
  await page.goto(LANDING);
  await page.locator("#board-demo").focus();
  // Closed, DONE shows its heading and count but no done rows.
  const heading = page.locator(".tsk-sec[data-drawer]");
  await expect(heading).toContainText("DONE");
  await expect(heading).toContainText("2 · z to open");
  const doneRow = page
    .locator("[data-task]")
    .filter({ hasText: "Add a health check for the worker pool" });
  await expect(doneRow).toHaveCount(0);
  await heading.click();
  await expect(page.locator(".tsk-sec[data-drawer]")).toHaveCount(0);
  await expect(doneRow).toHaveCount(1);
  await page.locator("[data-archived-header]").click();
  await expect(
    page
      .locator("[data-task]")
      .filter({ hasText: "Prototype a GraphQL gateway" }),
  ).toHaveCount(1);
  await page.locator("[data-scope]").click();
  await page.locator('[data-pick="website"]').click();
  await page.keyboard.press("z");
  await expect(
    page
      .locator("[data-task]")
      .filter({ hasText: "Prototype a GraphQL gateway" }),
  ).toHaveCount(0);
  await expect(page.locator("[data-archived-header]")).toHaveCount(0);
});

test("scope picker lists all, desk, and the projects and opens the choice", async ({
  page,
}) => {
  await page.goto(LANDING);
  await page.locator("[data-scope]").click();
  expect(
    await page
      .locator("[data-pick]")
      .evaluateAll((rows) => rows.map((row) => row.dataset.pick)),
  ).toEqual(["all", "desk", "launchpad", "website"]);
  await page.locator('[data-pick="website"]').click();
  await expect(page.locator("[data-scope]")).toHaveText("website ▾");
  await expect(page.locator('[role="dialog"]')).toHaveCount(0);
});

test("the questions tab ignores p, S, and search", async ({ page }) => {
  await page.goto(LANDING);
  await page.locator("#board-demo").focus();
  await page.keyboard.press("2");
  for (const key of ["p", "S", "/"]) {
    await page.keyboard.press(key);
    await expect(page.locator('[role="dialog"]')).toHaveCount(0);
    await expect(page.locator("#tsk-search")).toHaveCount(0);
  }
  await expect(page.locator("[data-scope]")).toHaveCount(0);
});
