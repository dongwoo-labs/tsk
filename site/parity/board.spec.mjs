import { readReference, referencePath } from "./reference.mjs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { test, expect } from "@playwright/test";
test.beforeAll(async ({ browser }) => {
  const root = new URL("../../", import.meta.url);
  const files = [
    "site/public/board-demo.js",
    "site/public/board-wrap.js",
    "site/public/task-steps.js",
    "site/public/capture.js",
    "site/public/landing.js",
    "site/src/styles/landing.css",
    "tests/fixtures/demo-parity/store.json",
  ];
  const hashes = Object.fromEntries(
    await Promise.all(
      files.map(async (path) => [
        path,
        createHash("sha256")
          .update(await readFile(new URL(path, root)))
          .digest("hex"),
      ]),
    ),
  );
  await writeFile(
    referencePath("browser-provenance.json"),
    JSON.stringify(
      {
        kind: "actual Chrome screenshots cropped to board root; not approved baselines",
        browser: browser.version(),
        head: execFileSync("git", ["rev-parse", "HEAD"], {
          encoding: "utf8",
        }).trim(),
        diff: execFileSync("git", ["diff", "--stat"], { encoding: "utf8" }),
        hashes,
      },
      null,
      2,
    ),
  );
});
// The fixture page (port 4178) mirrors tests/demo_parity.rs: every project, no Agent questions.
const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const row = (page, n) => page.locator(`[data-task="${id(n)}"]`);
async function open(page, width) {
  page.on("pageerror", (error) => console.error(error));
  await page.goto("/");
  await page
    .locator("#tsk-demo")
    .evaluate((el, w) => (el.style.width = `${w}ch`), width);
  await expect(row(page, 12)).toBeVisible();
  await page.locator("#board-demo").focus();
}
async function capture(page, testInfo, name) {
  await page.locator("#tsk-demo").screenshot({
    path: testInfo.outputPath(`${name}.png`),
    animations: "disabled",
  });
}

for (const width of [78, 110]) {
  test(`root Escape releases demo focus without switching tabs at ${width} columns`, async ({
    page,
  }) => {
    for (const [tab, key] of [
      ["board", "1"],
      ["questions", "2"],
    ]) {
      await open(page, width);
      await page.keyboard.press(key);
      await expect(page.locator(`[data-tab="${tab}"]`)).toHaveClass(/is-on/);
      await page.keyboard.press("Escape");
      await expect(page.locator(`[data-tab="${tab}"]`)).toHaveClass(/is-on/);
      await expect(page.locator("#board-demo")).not.toBeFocused();
    }
  });
}

test("the tab row names only the two tabs and rows group by project", async ({
  page,
}) => {
  await open(page, 78);
  await expect(page.locator(".tsk-tabs")).toHaveText(
    " 1 board  ·  2 questions",
  );
  await expect(page.locator(".tsk-tab")).toHaveCount(2);
  await expect(page.locator("[data-scope], [data-sort]")).toHaveCount(0);
  await expect(
    page.locator(".tsk-sub-label").filter({ hasText: "desk" }),
  ).toHaveCount(1);
  await expect(
    page.locator(".tsk-sub-label").filter({ hasText: "tsk-parity" }),
  ).toHaveCount(2);
  await page.keyboard.press("p");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.keyboard.press("2");
  await expect(page.locator("#tsk-demo")).toContainText(
    "this sample board has no Agent questions",
  );
});

test("multi-select gates task marking and ctrl click stays ordinary", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Space");
  await row(page, 12).dispatchEvent("click", { ctrlKey: true });
  await expect(page.locator("#tsk-demo")).not.toContainText(
    "selected · esc clears",
  );
  await page.keyboard.press("Shift+M");
  await expect(page.locator("#tsk-demo")).toContainText("multi-select");
  await row(page, 12).click();
  await expect(page.locator("#tsk-demo")).toContainText(
    "1 selected · esc clears",
  );
  await page.keyboard.press("Shift+M");
  await expect(page.locator("#tsk-demo")).not.toContainText(
    "selected · esc clears",
  );
});

test("text entry keeps every key but Enter and Escape", async ({ page }) => {
  await open(page, 78);
  await page.keyboard.press("Shift+M");
  await page.keyboard.press("+");
  const input = page.locator("#tsk-add");
  await input.press("Shift+M");
  await input.press("d");
  await input.press("2");
  await expect(input).toHaveValue("Md2");
  await input.press("Escape");
  await expect(input).toHaveCount(0);
  await expect(row(page, 12)).toBeVisible();
});

test("marked task sets complete, delete, and undo as one demo action", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Shift+M");
  await page.keyboard.press("Shift+ArrowDown");
  await page.keyboard.press("Space");
  await expect(page.locator("#tsk-demo")).toContainText(
    "2 selected · esc clears",
  );
  await expect(row(page, 12).locator(".tsk-row-prefix")).toContainText("▪");
  await expect(row(page, 13).locator(".tsk-row-prefix")).toContainText("▪");
  await page.keyboard.press("d");
  await expect(row(page, 12)).toHaveCount(0);
  await expect(row(page, 13)).toHaveCount(0);
  await page.keyboard.press("u");
  await expect(row(page, 12)).toBeVisible();
  await expect(row(page, 13)).toBeVisible();

  await page.keyboard.press("Shift+M");
  await page.keyboard.press("Shift+ArrowDown");
  await page.keyboard.press("Space");
  await page.keyboard.press("x");
  await expect(page.locator("#tsk-demo")).toContainText(
    "press x again to delete 2 tasks",
  );
  await page.keyboard.press("x");
  await expect(page.locator("#tsk-demo")).toContainText(
    "deleted 2 tasks · u restores",
  );
  await page.keyboard.press("u");
  await expect(row(page, 12)).toBeVisible();
  await expect(row(page, 13)).toBeVisible();
});

test("task-page demo actions ignore board marks and use the open task", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Shift+M");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Space");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await page.keyboard.press("d");
  await page.keyboard.press("Escape");
  await expect(row(page, 12)).toHaveCount(0);
  await expect(row(page, 13)).toBeVisible();
  await expect(page.locator("#tsk-demo")).not.toContainText(
    "selected · esc clears",
  );
});

test("Escape closes Help, then the detail focus, then releases the board", async ({
  page,
}) => {
  await open(page, 110);
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(".tsk-task-header.is-bold")).toBeVisible();
  await page.keyboard.press("?");
  await page.keyboard.press("Escape");
  await expect(page.locator(".tsk-task-header.is-bold")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".tsk-task-header.is-bold")).toHaveCount(0);
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await expect(page.locator("#board-demo")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.locator("#board-demo")).not.toBeFocused();
});

for (const width of [40, 78, 109, 110]) {
  test(`desk start, open, back, and wrapping at ${width} columns`, async ({
    page,
  }, info) => {
    await open(page, width);
    await expect(page.locator(".tsk-row-glyph")).toHaveCount(0);
    await expect(page.locator(".tsk-sec").first()).toContainText("DOING");
    await expect(row(page, 12)).toContainText("▸  T12");
    await capture(page, info, "initial");
    await page.keyboard.press("ArrowDown");
    await expect(row(page, 13)).toHaveClass(/is-sel/);
    const lines = await row(page, 13)
      .locator(".tsk-title-line")
      .allTextContents();
    expect(lines.length).toBeGreaterThan(1);
    const appLines = await readReference(`title-${width}`);
    expect(lines.map((line) => line.trimEnd())).toEqual(appLines);
    expect(lines.join("")).toBe(
      "Read every word of this long title before starting the task so the continuation must align beneath its title and retain the final boundary marker END",
    );
    await page.keyboard.press("s");
    await expect(row(page, 13)).toHaveClass(/is-sel/);
    await expect(page.locator(".tsk-sec").first()).toContainText("2");
    await capture(page, info, "doing");
    await page.keyboard.press("Enter");
    await expect(page.locator(".tsk-task-column")).toHaveAttribute(
      "data-status",
      "doing",
    );
    await expect(page.locator(".tsk-page-notes")).toHaveText(
      "A plain note for the first matched task-page flow.",
    );
    await expect(page.locator('[data-ring="parent"]')).toContainText(
      "parent  none · P to set",
    );
    if (width < 110) await expect(page.locator(".tsk-list")).toHaveCount(0);
    else await expect(page.locator(".tsk-list")).toHaveCount(1);
    await capture(page, info, "page");
    await page.keyboard.press("Escape");
    await expect(row(page, 13)).toHaveClass(/is-sel/);
    await capture(page, info, "back");
  });
}

test("the 110-column boundary switches between one screen and two panes", async ({
  page,
}) => {
  await open(page, 109);
  await expect(page.locator(".tsk-wide-split")).toHaveCount(0);
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(".tsk-task-column")).toHaveCount(0);
  await page.locator("#tsk-demo").evaluate((el) => (el.style.width = "110ch"));
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await expect(page.locator(".tsk-task-column")).toContainText(
    "Confirm the wording before release.",
  );
  await page.keyboard.press("l");
  await expect(page.locator(".tsk-task-header.is-bold")).toBeVisible();
  await page.keyboard.press("h");
  await expect(page.locator(".tsk-task-header.is-bold")).toHaveCount(0);
});

test("J and K step through the list from the page and refuse at the ends", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Enter");
  await page.keyboard.press("K");
  await expect(page.locator("#tsk-demo")).toContainText(
    "first task in this list",
  );
  await page.keyboard.press("J");
  await expect(page.locator(".tsk-task-column")).toContainText("END");
});

for (const width of [110, 130]) {
  test(`row double-click focuses the detail at ${width} columns`, async ({
    page,
  }) => {
    await open(page, width);
    await row(page, 13).dblclick();
    await expect(page.locator(".tsk-task-header.is-bold")).toContainText(
      "Read every word",
    );
  });
}

async function expectFormRing(page, { title, notes, start = "title" }) {
  const column = page.locator(".tsk-task-column");
  await expect(column).toHaveAttribute("data-edit-field", start);
  if (start === "title") {
    await page.locator("#tsk-edit").fill(title);
    await page.keyboard.press("Tab");
    await expect(column).toHaveAttribute("data-edit-field", "notes");
  }
  await page.locator("#tsk-edit").fill(notes);
  await page.keyboard.press("Tab");
  const stepIds = await page
    .locator("[data-step]")
    .evaluateAll((rows) => rows.map((row) => row.getAttribute("data-step")));
  for (const id of stepIds) {
    await expect(column).toHaveAttribute("data-edit-target", id);
    await page.keyboard.press("Tab");
  }
  await expect(column).toHaveAttribute("data-edit-target", "add");
  await page.keyboard.press("Tab");
  await expect(column).toHaveAttribute("data-edit-field", "scope");
  await page.keyboard.press("Tab");
  await expect(column).toHaveAttribute("data-edit-field", "title");
  await page.keyboard.press("Shift+Tab");
  await expect(column).toHaveAttribute("data-edit-field", "scope");
  await page.keyboard.press("Shift+Tab");
  await expect(column).toHaveAttribute("data-edit-target", "add");
}

test("task page and expanded quick-add keep the reversible form ring without Thread", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Enter");
  await page.keyboard.press("e");
  await expectFormRing(page, {
    title: "Task title survives Tab",
    notes: "Task notes survive Tab",
  });
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await page.keyboard.press("+");
  await page.locator("#tsk-add").fill("Quick ring");
  await page.keyboard.press("Tab");
  await expect(
    page.locator("[data-task]").filter({ hasText: "Quick ring" }),
  ).toHaveCount(0);
  await expectFormRing(page, {
    notes: "Quick notes survive Tab",
    start: "notes",
  });
  await page.keyboard.press("Escape");
  await expect(page.locator("#tsk-add")).toHaveValue("Quick ring");
});

test("an empty title does not save", async ({ page }) => {
  await open(page, 78);
  await page.keyboard.press("Enter");
  const original = await page.locator(".tsk-page-title").textContent();
  await page.keyboard.press("e");
  await page.locator("#tsk-edit").fill("");
  await page.keyboard.press("Shift+Enter");
  await expect(page.locator("#tsk-demo")).toContainText("title needed");
  await page.keyboard.press("Escape");
  await expect(page.locator(".tsk-page-title")).toHaveText(original || "");
});

test("landing column readout excludes board padding", async ({ page }) => {
  await open(page, 78);
  await page.evaluate(() => {
    const frame = document.getElementById("board-demo");
    const split = document.createElement("div");
    split.dataset.split = "";
    split.style.width = "1200px";
    frame.before(split);
    split.append(frame);
    const readout = document.createElement("span");
    readout.dataset.cols = "";
    split.append(readout);
    document.getElementById("tsk-demo").style.padding = "20px";
  });
  await page.addScriptTag({ url: "/public/landing.js" });
  await expect(page.locator("[data-cols] b")).toHaveText("78");
});
