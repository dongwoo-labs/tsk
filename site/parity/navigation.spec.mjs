import { test, expect } from "@playwright/test";
// The built landing page (port 4180) runs the demo on its sample data: launched in launchpad, with
// sample Agent questions. Nothing in it is stored, sent, or approved.
const LANDING = "http://127.0.0.1:4180/";
const row = (page, n) => page.locator(`[data-task="t${n}"]`);

test("selection uses an arrow and an underlined tab without reverse fill", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.locator("[data-task]").first()).toContainText("▸  T12");
  await expect(page.locator("[data-task]").first()).toHaveCSS(
    "background-color",
    "rgba(0, 0, 0, 0)",
  );
  await expect(page.locator(".tsk-tab-group.is-on .tsk-tab")).toHaveCSS(
    "text-decoration-line",
    "underline",
  );
  const inactive = page.locator(".tsk-tab-group:not(.is-on)");
  await expect(inactive).toHaveCount(2);
  for (const tab of await inactive.all())
    await expect(tab).toHaveCSS("text-decoration-line", "none");
  await page.locator('[data-tab="questions"]').click();
  await expect(page.locator('[data-tab="questions"]')).toHaveClass(/is-on/);
});

test("full terminal opens with the list and the selected task side by side", async ({
  page,
}) => {
  await page.goto(LANDING);
  await expect(page.locator("[data-split]")).toHaveClass(/is-full/);
  await expect(page.locator('[data-layout="full"]')).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await expect(
    page.locator(".tsk-board-surface [data-task]").first(),
  ).toBeVisible();
  await expect(page.locator(".tsk-task-header")).toBeVisible();
});

test("the sample board shows every project grouped, cursor on launchpad", async ({
  page,
}) => {
  await page.goto(LANDING);
  await expect(page.locator("[data-scope], [data-sort]")).toHaveCount(0);
  await expect(page.locator(".tsk-sec").nth(1)).toHaveText(/^TODO/);
  for (const label of ["desk", "launchpad", "website"])
    await expect(
      page.locator(".tsk-sub-label").filter({ hasText: label }).first(),
    ).toBeVisible();
  for (const title of [
    "Make webhook delivery idempotent",
    "Trace worker queue saturation",
    "Plan the support handoff",
    "Record the retry runbook",
    "Review error states",
  ])
    await expect(
      page.locator("[data-task]").filter({ hasText: title }),
    ).toHaveCount(1);
  await expect(row(page, 12)).toContainText("▸  T12");
});

test("rows carry question marks and the child count; the waiting line opens tab 2", async ({
  page,
}) => {
  await page.goto(LANDING);
  await expect(row(page, 12).locator(".tsk-row-tail")).toHaveText(
    "! ?1 ✓1/3 ⊂1/2",
  );
  await expect(row(page, 13).locator(".tsk-row-tail")).toHaveText("?2 ✓0/3");
  // The notes' status card: a Doing task's Now line dim, a Blocked line bold.
  // A long line wraps under the title onto more than one row.
  expect(
    (await row(page, 12).locator(".tsk-row-status").allTextContents())
      .map((line) => line.trim())
      .join(" "),
  ).toBe("rotating tokens in the refresh endpoint");
  expect(
    (await row(page, 15).locator(".tsk-row-status").allTextContents())
      .map((line) => line.trim())
      .join(" "),
  ).toMatch(/^blocked: staging client registration/);
  await expect(page.locator("[data-waiting]")).toContainText(
    "waiting on you  approval 1 · question 2 · finding 1 · target unknown 1  → 2",
  );
  await expect(page.locator('[data-tab="questions"]')).toHaveText(
    /^2( questions)? ·4$/,
  );
  await page.locator("[data-waiting]").click();
  await expect(page.locator('[data-tab="questions"]')).toHaveClass(/is-on/);
  await expect(page.locator(".tsk-q-group")).toHaveText([
    " NEEDS APPROVAL 1",
    " FOR YOU 2",
    " FINDINGS 1",
    " WITH LEAD 1",
    " target unknown · route with tsk route 1",
    " CLOSED 1 · z shows",
  ]);
});

test("an approval is a numbered form, 1 approves, and is sample only", async ({
  page,
}) => {
  await page.goto(LANDING);
  await page.locator("#board-demo").focus();
  await page.keyboard.press("2");
  await page.keyboard.press("Enter");
  const form = page.locator(".tsk-q-page");
  await expect(form).toContainText("▸ 1. Approve these exact terms");
  await expect(form).toContainText("2. Reject");
  await expect(form).toContainText("3. Add a comment…");
  // The old check and decide keys are gone: nothing is saved.
  await page.keyboard.press("Space");
  await page.keyboard.press("y");
  await expect(form).toContainText("Q1 · approval");
  await page.keyboard.press("1");
  await expect(page.locator("#tsk-demo")).toContainText("Q1 approved · sample");
  await expect(page.locator('[data-question="q2"]')).toContainText("▸ Q2");
  await expect(page.locator(".tsk-q-group").last()).toHaveText(
    " CLOSED 2 · z shows",
  );
  await page.keyboard.press("z");
  await page.locator('[data-question="q1"]').click();
  await expect(page.locator(".tsk-q-page")).toContainText(
    "sample: approved in this demo only · it authorizes nothing",
  );
});

test("answers, a finding, J/K, t, and the unrouted refusal", async ({
  page,
}) => {
  await page.goto(LANDING);
  await page.locator("#board-demo").focus();
  await page.keyboard.press("2");
  await page.locator('[data-question="q2"]').click();
  await expect(page.locator(".tsk-q-line.is-chosen")).toContainText(
    "postgres table · recommended",
  );
  await page.keyboard.press("Enter");
  await expect(page.locator("#tsk-demo")).toContainText("Q2 saved · sample");
  await expect(page.locator('[data-question="q3"]')).toContainText("▸ Q3");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter");
  const answer = page.locator("#tsk-answer");
  await answer.press("y");
  await answer.press("n");
  await answer.press("c");
  await expect(answer).toHaveValue("ync");
  await answer.press("Enter");
  await expect(page.locator("#tsk-demo")).toContainText("Q3 saved · sample");
  await page.locator("#board-demo").focus();
  await page.keyboard.press("z");
  await page.locator('[data-question="q2"]').click();
  await expect(page.locator(".tsk-q-page")).toContainText(
    "answer  postgres table",
  );
  await page.locator('[data-question="q3"]').click();
  await expect(page.locator(".tsk-q-page")).toContainText("answer  ync");

  await page.locator('[data-question="q4"]').click();
  await page.keyboard.press("Enter");
  await expect(page.locator("#tsk-demo")).toContainText("Q4 saved · sample");
  await page.locator('[data-question="q4"]').click();
  await expect(page.locator(".tsk-q-page")).toContainText(
    "no run request is sent",
  );
  const created = await page.locator(".tsk-q-page").textContent();
  const number = created.match(/created (T\d+) \(doing\)/)[1];
  await page.keyboard.press("c");
  await page.keyboard.press("j");
  await page.keyboard.press("Enter");
  await page.locator('[data-question="q4"]').click();
  await expect(page.locator(".tsk-q-page")).toContainText(
    `the linked task ${number} stays · no second task`,
  );

  await page.locator('[data-question="q6"]').click();
  await page.keyboard.press("Enter");
  await expect(page.locator("#tsk-demo")).toContainText(
    "target unknown · route it with tsk route first",
  );
  await page.keyboard.press("K");
  await expect(page.locator(".tsk-q-page")).toContainText("Q5 · question");
  // Q5 has no options: it opens on its focused field, as in the app, so `t` is text
  // there until Esc leaves the field.
  await expect(page.locator("#tsk-answer")).toBeFocused();
  await page.locator("#tsk-answer").press("Escape");
  await page.keyboard.press("t");
  await expect(page.locator('[data-tab="board"]')).toHaveClass(/is-on/);
  await expect(page.locator(".tsk-task-column")).toContainText(
    "Add cursor pagination",
  );
  await page.keyboard.press("Escape");
  await expect(page.locator('[data-tab="questions"]')).toHaveClass(/is-on/);
  await expect(page.locator(".tsk-q-page")).toContainText("Q5 · question");
});

test("the task page lists children, questions, and links, and Esc walks back", async ({
  page,
}) => {
  await page.goto(LANDING);
  await page.locator('[data-layout="beside"]').click();
  await page.locator("#board-demo").focus();
  await page.keyboard.press("Enter");
  const column = page.locator(".tsk-task-column");
  await expect(column).toContainText("children 1/2 done");
  await expect(column).toContainText("questions 2 open");
  await expect(column).toContainText("thread thr-launch-12 · enter focuses");
  await page.locator('[data-ring="child:t15"]').click();
  await expect(column).toContainText("Unblock OIDC login in staging");
  await expect(column).toContainText("↑ T12 Rotate refresh tokens");
  await page.keyboard.press("Escape");
  await expect(column).toContainText("Rotate refresh tokens on every use");
  await page.locator('[data-ring="link:1"]').click();
  await expect(page.locator("#tsk-demo")).toContainText(
    "sample: the demo has no Herdr · nothing focused",
  );
  await page.locator('[data-ring="q:q1"]').click();
  await expect(page.locator(".tsk-q-page")).toContainText("Q1 · approval");
  await page.keyboard.press("Escape");
  await expect(column).toContainText("Rotate refresh tokens on every use");
});
