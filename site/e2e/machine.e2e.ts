// Disconnected sample acceptance. No task API, hostname inference or real sends.
// Build site/dist first. Use a task-local native-e2e config and a fresh browser.
// The live CLI journey is recorded under .artifacts/logs/T4239/acceptance.
import { createRequire } from "node:module";

if (!process.env.NATIVE_E2E_ROOT) throw new Error("NATIVE_E2E_ROOT is required");
const require = createRequire(`${process.env.NATIVE_E2E_ROOT}/package.json`);
const { test, expect } = await import(require.resolve("e2e"));

test("machine ranges filter the disconnected sample without changing task identity", async ({ app, screen }) => {
  await app.open("/");
  const demo = screen.getByRole("application", "tsk board live demo");
  await demo.getByRole("button", "full terminal").tap();
  await expect(demo.getByRole("button", "machine @local · all")).toBeVisible();
  await expect(demo.getByRole("button", "2 questions ·4")).toBeVisible();
  await expect(demo.getByRole("button", "3 reports ·1")).toBeVisible();
  await app.screenshot("machine-local-wide");

  const scope = async (name: string) => {
    await demo.press(":");
    await demo.getByText(`machine: ${name}`).tap();
    await expect(demo.getByRole("button", `machine ${name} · all`)).toBeVisible();
  };
  await scope("@current");
  await expect(demo.getByText("no task")).toBeVisible();
  await expect(demo.getByRole("button", "2 questions")).toBeVisible();
  await expect(demo.getByRole("button", "3 reports")).toBeVisible();
  await app.screenshot("machine-current-empty");
  await demo.getByRole("button", "+ add").tap();
  const title = demo.getByRole("textbox", "title  ·  !p project");
  await title.fill("Machine acceptance sample");
  await title.press("Enter");
  await expect(demo.getByRole("button", "▸ T25 Machine acceptance sample [demo]")).toBeVisible();
  await expect(demo.getByText("⎇ main (default) · machine demo")).toBeVisible();
  await app.screenshot("machine-current-created");

  await demo.getByRole("button", "beside your agent").tap();
  await expect(demo.getByText("78 cols · one screen")).toBeVisible();
  await app.screenshot("machine-current-narrow");
  await scope("@unspecified");
  await expect(demo.getByRole("button", "2 questions ·4")).toBeVisible();
  await expect(demo.getByRole("button", "3 reports ·1")).toBeVisible();
  await demo.press("2");
  await expect(demo.getByText("NEEDS APPROVAL 1")).toBeVisible();
  await expect(demo.getByRole("button", "machine @unspecified · questions · 4 waiting · 7 total · sample")).toBeVisible();
  // Inspect this capture: the machine selector at the start of the footer must
  // remain legible at 78 columns. DOM presence alone does not prove that.
  await app.screenshot("machine-unspecified-questions-narrow");
  await demo.press("3");
  await expect(demo.getByRole("button", "▸ T20 Add a health check for the worker pool · revision 1")).toBeVisible();
  await app.screenshot("machine-unspecified-reports-narrow");
  await demo.press("1");
  await expect(demo.getByRole("button", "DONE 2 · z to open")).toBeVisible();
  await demo.press("z");
  await app.screenshot("machine-unspecified-drawer");
  await scope("demo");
  await expect(demo.getByRole("button", "▸ T25 Machine acceptance sample [demo]")).toBeVisible();
  await app.screenshot("machine-literal-demo");
  await scope("@all");
  await expect(demo.getByRole("button", "2 questions ·4")).toBeVisible();
  await expect(demo.getByRole("button", "3 reports ·1")).toBeVisible();
  await scope("@local");
  await expect(demo.getByRole("button", "2 questions ·4")).toBeVisible();
  await app.screenshot("machine-local-restored");
});
