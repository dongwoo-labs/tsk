// This journey tests the disconnected website sample, not Rust persistence.
// A fresh browser loads the built demo's own sample. External fetch is blocked
// by the acceptance config; no task API or provider is connected.
import { createRequire } from "node:module";

if (!process.env.NATIVE_E2E_ROOT)
  throw new Error("NATIVE_E2E_ROOT is required");
const require = createRequire(`${process.env.NATIVE_E2E_ROOT}/package.json`);
const { test, expect } = await import(require.resolve("e2e"));

test("reports stay unread until opened and remain accessible after reading", async ({
  app,
  screen,
}) => {
  await app.open("/");
  await app.screenshot("hero-footer");
  const demo = screen.getByRole("application", "tsk board live demo");
  await demo.getByRole("button", "3 reports ·1").tap();
  await expect(demo.getByText("UNREAD")).toBeVisible();
  await expect(demo.getByRole("button", "3 reports ·1")).toBeVisible();
  const report = demo.getByRole(
    "button",
    "▸ T20 Add a health check for the worker pool · revision 1",
  );
  await expect(report).toBeVisible();
  await app.screenshot("unread-selected");

  await report.tap();
  await expect(
    demo.getByText("T20 Add a health check for the worker pool"),
  ).toBeVisible();
  await expect(demo.getByText("Worker health check")).toBeVisible();
  await expect(demo.getByText("Healthy worker │ PASS")).toBeVisible();
  await expect(demo.getByText("curl /health/ready")).toBeVisible();
  await expect(demo.getByRole("button", "3 reports")).toBeVisible();
  await demo.getByLabel("Completion report").scrollIntoView();
  await app.screenshot("opened-health-report");
  await demo
    .getByText("No live integration was exercised in this demo.")
    .scrollIntoView();
  await app.screenshot("report-bottom");

  await demo.press("Escape");
  await expect(demo.getByText("READ")).toBeVisible();
  await expect(report).toBeVisible();
  await app.screenshot("read-list");
  await demo.press("Enter");
  await expect(demo.getByText("Worker health check")).toBeVisible();
  await demo.press("2");
  await expect(demo.getByText("NEEDS APPROVAL 1")).toBeVisible();
  await app.screenshot("wide-questions");
  await demo.press("1");
  await expect(demo.getByText("DOING")).toBeVisible();
  await demo.getByRole("button", "full terminal").tap();
  await app.screenshot("wide-board");
  await demo.getByRole("button", "beside your agent").tap();
  await expect(demo.getByText("78 cols · one screen")).toBeVisible();
  await app.screenshot("narrow-board");
  await demo.getByRole("button", "+ add").tap();
  const title = demo.getByRole("textbox", "title  ·  !p project");
  await title.fill("Sample acceptance !p website !t literal");
  await title.press("Enter");
  await expect(demo.getByRole("button", "▸ T25 Sample acceptance !t literal")).toBeVisible();
  await app.screenshot("narrow-literal-title");
});
