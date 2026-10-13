import { TaskSteps } from "./task-steps.js";
import { wrapText } from "./board-wrap.js";
import { parseCapture } from "./capture.js";

// The landing page's board: sample data only, disconnected from any store, Herdr, or Lead.
// It follows the app's screens and keys, except that status verbs are bare letters
// (s doing, o todo, d done, x delete, u undo, f archive) because browsers keep the
// control chords. Nothing here is saved, sent, or approved.
(() => {
  const root = document.getElementById("tsk-demo");
  const frame = document.getElementById("board-demo");
  if (!root || !frame) return;

  const WIDE_SPLIT_MIN_COLUMNS = 110;
  const GLYPH = { todo: "○", doing: "●", done: "✓" };

  // Optional fixture data is supplied only by the local parity harness.
  const fixture = JSON.parse(
    document.getElementById("tsk-demo-fixture")?.textContent || "null",
  );
  const clock = () => fixture?.now ?? Date.now();
  const launchProject = () => fixture?.selectedProject || "launchpad";
  const NOW = clock();
  const MIN = 60 * 1000;

  const esc = (s) =>
    String(s).replace(
      /[&<>"']/g,
      (c) =>
        ({
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        })[c],
    );
  const age = (ts) => {
    const secs = Math.max(0, Math.floor((clock() - ts) / 1000));
    if (secs < 60) return `${secs}s`;
    if (secs < 3600) return `${Math.floor(secs / 60)}m`;
    if (secs < 86400) return `${Math.floor(secs / 3600)}h`;
    return `${Math.floor(secs / 86400)}d`;
  };
  const projectName = (task) => task.project || "desk";

  // ── sample data ───────────────────────────────────────────────────────────
  const seed = () => {
    let n = 12;
    const task = (partial) => ({
      id: `t${n}`,
      number: n++,
      notes: "",
      steps: [],
      project: null,
      parent: null,
      archived: false,
      createdAt: NOW - (40 - n) * MIN,
      updatedAt: NOW - 3 * MIN,
      ...partial,
    });
    const steps = (prefix, rows) =>
      rows.map(([text, done], i) => ({ id: `${prefix}-${i}`, text, done }));
    return [
      task({
        title: "Rotate refresh tokens on every use",
        status: "doing",
        project: "launchpad",
        notes:
          "Store a hash of each refresh token and revoke its token family if an old token is reused.\nLinear: LAUNCH-12 https://linear.app/example/issue/LAUNCH-12\nThread: thr-launch-12 w1 t2 p3",
        steps: steps("s12", [
          ["Add token-family storage", true],
          ["Rotate tokens in the refresh endpoint", false],
          ["Test replay and expiry", false],
        ]),
      }),
      task({
        title: "Make webhook delivery idempotent",
        status: "todo",
        project: "launchpad",
        notes:
          "Retries can deliver the same event twice. Deduplicate by event ID before applying side effects.\nPR: https://github.com/example/launchpad/pull/88",
        steps: steps("s13", [
          ["Reproduce duplicate delivery", false],
          ["Persist processed event IDs", false],
          ["Test concurrent retries", false],
        ]),
      }),
      task({
        title: "Investigate slow integration tests",
        status: "todo",
        notes:
          "The API suite takes twelve minutes in CI. Measure where the time goes before changing the runner.",
        steps: steps("s14", [
          ["Capture per-test timings", false],
          ["Find repeated database setup", false],
        ]),
      }),
      task({
        title: "Unblock OIDC login in staging",
        status: "doing",
        project: "launchpad",
        parent: "t12",
        notes:
          "Waiting for the identity provider's staging client registration. The callback handler is ready.",
        steps: steps("s15", [
          ["Verify the redirect URI", true],
          ["Register the staging client", false],
        ]),
      }),
      task({
        title: "Review API key scope checks",
        status: "done",
        project: "launchpad",
        parent: "t12",
        notes:
          "The middleware checks scopes before routing. Logs never include credentials.",
      }),
      task({
        title: "Generate the TypeScript API client",
        status: "doing",
        project: "website",
        notes:
          "Waiting for the pagination schema before generating the client used by the console.",
      }),
      task({
        title: "Review error states in the API console",
        status: "todo",
        project: "website",
        notes:
          "Check how the console handles expired sessions, rate limits, and server errors without losing request input.",
      }),
      task({
        title: "Add cursor pagination to the events endpoint",
        status: "todo",
        project: "launchpad",
        notes:
          "Use a stable cursor for events with matching timestamps. Keep the response contract backwards compatible.",
      }),
      task({
        title: "Add a health check for the worker pool",
        status: "done",
        project: "launchpad",
        notes:
          "Readiness fails when workers stop consuming jobs; liveness stays independent.",
      }),
      task({
        title: "Prototype a GraphQL gateway",
        status: "todo",
        project: "launchpad",
        notes:
          "Keep the prototype for reference while the REST API stabilizes.",
        archived: true,
      }),
      task({
        title: "Plan the support handoff",
        status: "todo",
        notes:
          "Pick the owner, escalation path, and first response expectations before the pilot opens.",
      }),
      task({
        title: "Record the retry runbook",
        status: "todo",
        notes:
          "Capture the safe retry steps while the incident details are still fresh.",
      }),
      task({
        title: "Trace worker queue saturation",
        status: "todo",
        project: "launchpad",
        notes:
          "Measure queue depth and worker lag before choosing a scaling threshold.",
      }),
    ];
  };

  // Sample agent questions. Nothing here is a real private record or a real target.
  const seedQuestions = () => {
    const q = (n, partial) => ({
      id: `q${n}`,
      number: n,
      options: [],
      recommendation: null,
      audience: "user",
      leadRecipient: null,
      approval: null,
      finding: null,
      blocks: null,
      answers: [],
      origin: "thr-launch-12",
      ...partial,
    });
    return [
      q(1, {
        taskId: "t12",
        kind: "approval",
        summary: "Deploy token rotation to staging",
        text: "The rotation passes its tests. May I deploy it to staging?",
        blocks: "staging rollout only",
        approval: {
          action: "deploy token rotation",
          target: "staging",
          scope: "launchpad api",
          h: "4f2c9a1e7b3d5c8a0f6e2d4b1a9c7e5f3d2b1a08",
          rollback: "redeploy the previous build",
        },
      }),
      q(2, {
        taskId: "t13",
        kind: "question",
        origin: "thr-launch-13",
        summary: "Where should processed event IDs live?",
        text: "The dedupe check needs a store. Which one fits?",
        options: ["postgres table", "redis set with a TTL"],
        recommendation: "postgres table",
      }),
      q(3, {
        taskId: "t12",
        kind: "question",
        summary: "Name the token-family table",
        text: "What should the new table be called?",
      }),
      q(4, {
        taskId: "t13",
        kind: "finding",
        origin: "thr-launch-13",
        summary: "Retries skip the dead-letter queue",
        text: "A failed retry is dropped instead of parked.",
        leadRecipient: "lead-demo",
        finding: {
          kind: "issue",
          detail: "Events that fail three times are lost.",
          path: "src/webhooks/retry.ts",
        },
      }),
      q(5, {
        taskId: "t19",
        kind: "question",
        audience: "lead",
        origin: "thr-launch-19",
        summary: "Pick a reviewer for the cursor change",
        text: "Who reviews the pagination change?",
      }),
      q(6, {
        taskId: "t14",
        kind: "question",
        audience: null,
        origin: "thr-ci",
        summary: "Legacy question without a target",
        text: "An older record that names no target.",
      }),
      q(7, {
        taskId: "t16",
        kind: "question",
        summary: "Log scope denials?",
        text: "Should denied requests be logged?",
        answers: [
          { actor: "user", text: "yes, without the key", action: null },
        ],
      }),
    ];
  };

  const state = {
    tasks: fixture ? structuredClone(fixture.tasks) : seed(),
    // null: no Agent questions on this board (the parity fixture), as on a personal board.
    questions: fixture ? (fixture.questions ?? null) : seedQuestions(),
    tab: "board",
    scope: fixture?.scope ?? launchProject(),
    sort: "number",
    searchQuery: "",
    searchPinned: false,
    selectedId: null,
    markMode: false,
    markedIds: new Set(),
    pendingDelete: null,
    pendingDeleteBulk: false,
    message: "",
    flashId: null,
    copyNotice: "",
    drawer: false,
    archivedOpen: false,
    overlay: null,
    draft: "",
    paletteQ: "",
    paletteI: 0,
    helpQ: "",
    pickerI: 0,
    parentQ: "",
    parentI: 0,
    // Narrow: the task page owns the screen. Wide: the detail beside the list has focus.
    page: false,
    trail: [],
    edit: null,
    quickExpanded: false,
    capture: null,
    refuse: "",
    undo: null,
    nextId: 40,
    nextNumber: fixture ? 23 : 25,
    q: {
      cursor: null,
      page: null,
      options: {},
      check: null,
      answer: null,
      correcting: null,
      report: {},
      returnTask: null,
      returnQuestion: null,
      returnDepth: 0,
      closedOpen: false,
    },
  };
  const steps = new TaskSteps();

  // ── geometry ──────────────────────────────────────────────────────────────
  let renderColumns = null;
  function terminalColumns() {
    if (renderColumns !== null) return renderColumns;
    const style = getComputedStyle(root);
    const context = document.createElement("canvas").getContext("2d");
    context.font = `${style.fontSize} ${style.fontFamily}`;
    const cell = context.measureText("0").width;
    const padding =
      parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    return Math.round((root.clientWidth - padding) / cell);
  }
  const isWide = () => terminalColumns() >= WIDE_SPLIT_MIN_COLUMNS;
  const listColumns = () =>
    isWide() ? Math.floor(terminalColumns() * 0.4) : terminalColumns();
  const detailColumns = () =>
    isWide() ? terminalColumns() - listColumns() - 2 : terminalColumns();

  // ── tasks ─────────────────────────────────────────────────────────────────
  const taskById = (id) => state.tasks.find((t) => t.id === id);
  const statusAt = (t) => t.statusAt ?? t.createdAt;
  const byNumber = (a, b) => (a.number ?? 1e9) - (b.number ?? 1e9);
  const byStatusChange = (a, b) =>
    statusAt(b) - statusAt(a) || String(a.id).localeCompare(String(b.id));
  const children = (task) => state.tasks.filter((t) => t.parent === task.id);

  function projectNames() {
    const names = new Set();
    for (const t of state.tasks) if (t.project) names.add(t.project);
    return [...names].sort();
  }
  const scopeOptions = () => ["all", "desk", ...projectNames()];

  function inScope(task) {
    if (state.scope === "all") return true;
    if (state.scope === "desk") return !task.project || task.status === "doing";
    return task.project === state.scope;
  }

  function taskMatchesSearch(task, query) {
    const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return true;
    const content = [
      task.title,
      task.notes,
      ...(task.steps || []).map((step) => step.text),
      `T${task.number}`,
    ]
      .filter(Boolean)
      .join("\n")
      .toLowerCase();
    return words.every((word) => content.includes(word));
  }

  function buildRows() {
    const rows = [];
    const live = state.tasks.filter((t) => !t.archived && inScope(t));
    const match = (t) => taskMatchesSearch(t, state.searchQuery);
    const section = (label, tasks, kind) => {
      if (!tasks.length && state.searchQuery.trim()) return;
      if (!tasks.length && kind !== "todo") return;
      rows.push({ kind: "section", label, count: tasks.length });
      if (state.sort === "project") {
        const groups = new Map();
        for (const t of tasks) {
          const key = projectName(t);
          if (!groups.has(key)) groups.set(key, []);
          groups.get(key).push(t);
        }
        const keys = [...groups.keys()].sort((a, b) =>
          a === "desk" ? -1 : b === "desk" ? 1 : a.localeCompare(b),
        );
        for (const key of keys) {
          rows.push({ kind: "sub", label: key });
          for (const t of groups.get(key).sort(byNumber))
            rows.push({ kind: "task", task: t, id: t.id, selectable: true });
        }
      } else {
        for (const t of tasks.sort(byNumber))
          rows.push({ kind: "task", task: t, id: t.id, selectable: true });
      }
    };
    section(
      "DOING",
      live.filter((t) => t.status === "doing" && match(t)),
      "doing",
    );
    section(
      state.scope === "desk" ? "TODO · desk" : "TODO",
      live.filter((t) => t.status === "todo" && match(t)),
      "todo",
    );
    const done = live
      .filter((t) => t.status === "done" && match(t))
      .sort(byStatusChange);
    // Closed, DONE keeps its heading and count with no rows, as in the app.
    if (!state.drawer && done.length)
      rows.push({
        kind: "section",
        label: "DONE",
        count: done.length,
        collapsed: true,
      });
    if (state.drawer) {
      if (done.length) {
        rows.push({ kind: "section", label: "DONE", count: done.length });
        for (const t of done)
          rows.push({ kind: "task", task: t, id: t.id, selectable: true });
      }
      const archived = state.tasks.filter(
        (t) => t.archived && inScope(t) && match(t),
      );
      if (archived.length) {
        rows.push({
          kind: "archived",
          count: archived.length,
          id: "nav:archived",
          selectable: true,
        });
        if (state.archivedOpen)
          for (const t of archived)
            rows.push({
              kind: "task",
              task: t,
              id: t.id,
              selectable: true,
              dim: true,
            });
      }
    }
    return rows;
  }
  const selectableIds = (rows = buildRows()) =>
    rows.filter((r) => r.selectable).map((r) => r.id);
  const listTaskIds = () =>
    buildRows()
      .filter((r) => r.kind === "task")
      .map((r) => r.id);

  function ensureSelection(rows = buildRows()) {
    const ids = selectableIds(rows);
    if (!ids.length) state.selectedId = null;
    else if (!ids.includes(state.selectedId)) state.selectedId = ids[0];
  }

  // The task the page or detail shows: a related task may sit outside the list.
  function shownTask() {
    if (state.quickExpanded) return state.capture;
    return taskById(state.trail.at(-1) ?? state.selectedId) || null;
  }
  const selectedRow = () =>
    buildRows().find((r) => r.id === state.selectedId) || null;
  const hasUnsavedWork = () =>
    Boolean(state.edit || steps.dirty || steps.editor);

  // ── questions ─────────────────────────────────────────────────────────────
  const questionsLoaded = () => Array.isArray(state.questions);
  const questionById = (id) =>
    (state.questions || []).find((q) => q.id === id) || null;
  const isOpen = (q) => q.answers.length === 0;
  const currentAnswer = (q) => q.answers.at(-1) || null;
  const GROUPS = [
    ["approval", "NEEDS APPROVAL"],
    ["foryou", "FOR YOU"],
    ["findings", "FINDINGS"],
    ["lead", "WITH LEAD"],
    ["unrouted", "target unknown · route with tsk route"],
    ["closed", "CLOSED"],
  ];
  function groupOf(q) {
    if (!isOpen(q)) return "closed";
    if (!q.audience) return "unrouted";
    if (q.audience === "lead") return "lead";
    return q.kind === "approval"
      ? "approval"
      : q.kind === "finding"
        ? "findings"
        : "foryou";
  }
  function grouped() {
    return GROUPS.map(([key, title]) => [
      key,
      title,
      (state.questions || [])
        .filter((q) => groupOf(q) === key)
        .sort((a, b) => a.number - b.number),
    ]).filter(([, , members]) => members.length);
  }
  // A folded CLOSED group lists none of its records, so j/k and J/K skip them.
  const questionOrder = () =>
    grouped()
      .filter(([key]) => key !== "closed" || state.q.closedOpen)
      .flatMap(([, , members]) => members.map((q) => q.id));
  function toggleClosed() {
    state.q.closedOpen = !state.q.closedOpen;
    const order = questionOrder();
    if (!order.includes(state.q.cursor)) state.q.cursor = order[0] ?? null;
  }
  // Leave the page: back to the task page it came from, else to the list.
  function leaveQuestionPage() {
    if (state.q.returnTask && taskById(state.q.returnTask)) {
      const task = taskById(state.q.returnTask);
      state.q.returnTask = null;
      state.q.page = null;
      state.tab = "board";
      if (listTaskIds().includes(task.id)) {
        state.selectedId = task.id;
        state.trail = [];
      } else state.trail = [task.id];
      state.page = true;
      steps.reset();
    } else state.q.page = null;
  }
  // A saved answer returns to the list; the cursor keeps the record's place, now held by
  // the next record still waiting.
  function backToList(q, slot) {
    leaveQuestionPage();
    // A record that just closed gives its place to the waiting row now in its slot.
    const waiting = grouped()
      .filter(([key]) => key !== "closed")
      .flatMap(([, , members]) => members.map((r) => r.id));
    state.q.cursor = waiting.includes(q.id)
      ? q.id
      : (waiting[Math.min(slot, waiting.length - 1)] ??
        questionOrder()[0] ??
        null);
  }
  function waiting() {
    const out = { approval: 0, question: 0, finding: 0, unrouted: 0 };
    for (const q of state.questions || []) {
      const g = groupOf(q);
      if (g === "approval") out.approval++;
      else if (g === "foryou") out.question++;
      else if (g === "findings") out.finding++;
      else if (g === "unrouted") out.unrouted++;
    }
    return out;
  }
  const forYou = (w = waiting()) => w.approval + w.question + w.finding;
  function waitingLine() {
    if (!questionsLoaded()) return null;
    const w = waiting();
    const parts = ["approval", "question", "finding"]
      .filter((k) => w[k])
      .map((k) => `${k} ${w[k]}`);
    if (w.unrouted) parts.push(`target unknown ${w.unrouted}`);
    return parts.length ? `waiting on you  ${parts.join(" · ")}  → 2` : null;
  }
  function taskMarks(task) {
    let approval = false,
      n = 0;
    for (const q of state.questions || []) {
      if (q.taskId !== task.id) continue;
      const g = groupOf(q);
      if (g === "approval") approval = true;
      else if (g === "foryou" || g === "findings") n++;
    }
    if (approval && n) return `! ?${n}`;
    if (approval) return "!";
    return n ? `?${n}` : "";
  }
  const taskLabel = (id) => {
    const t = taskById(id);
    return t ? `T${t.number}` : "a missing task";
  };
  function stateLabel(q) {
    const answer = currentAnswer(q);
    if (!answer)
      return !q.audience
        ? "open · target unknown"
        : q.audience === "lead"
          ? "open · with lead"
          : "open · for you";
    if (q.kind === "approval") return answer.approved ? "approved" : "rejected";
    if (q.kind === "finding") {
      if (q.createdTask) return `→ ${taskLabel(q.createdTask)}`;
      return answer.action === "close" ? "closed" : "answered";
    }
    return "answered";
  }
  const FINDING_ACTIONS = [
    ["create_and_run", "create the task and run it"],
    ["create_only", "create the task only (todo)"],
    ["close", "close · no task"],
  ];
  const choices = (q) =>
    q.kind === "finding" ? FINDING_ACTIONS.map(([, l]) => l) : q.options;
  function choice(q) {
    const list = choices(q);
    const fallback =
      q.kind === "finding"
        ? 0
        : Math.max(0, q.options.indexOf(q.recommendation));
    return Math.min(state.q.options[q.id] ?? fallback, list.length - 1);
  }
  const choosing = (q) =>
    q.kind !== "approval" &&
    choices(q).length > 0 &&
    ((isOpen(q) && q.audience) || state.q.correcting === q.id);
  const checked = (q) =>
    state.q.check?.id === q.id && state.q.check.revision === q.answers.length;
  const kindWord = (q) =>
    q.kind === "finding" ? `finding · ${q.finding?.kind || "issue"}` : q.kind;
  const audienceWord = (q) =>
    !q.audience ? "target unknown" : q.audience === "lead" ? "lead" : "you";

  function questionPageRows(q, width) {
    const rows = [];
    const push = (text, cls = "", attrs = "") =>
      rows.push({ text, cls, attrs });
    const indented = (lead, text, cls = "", attrs = "") => {
      wrapText(text, Math.max(1, width - lead.length)).forEach((line, i) =>
        push((i ? " ".repeat(lead.length) : lead) + line, cls, attrs),
      );
    };
    const blank = () => push("");
    // A `label  value` field: the label paints dim so the value reads first.
    const field = (lead, text, cls = "", attrs = "") => {
      const at = rows.length;
      indented(lead, text, cls, attrs);
      if (rows[at]) rows[at].label = lead.length;
    };
    const task = taskById(q.taskId);
    push(`Q${q.number} · ${kindWord(q)} · ${stateLabel(q)}`, "is-bold");
    field(
      "task    ",
      task
        ? `T${task.number} ${task.title} · t opens`
        : `${q.taskId} not found`,
      "",
      'data-q-key="t"',
    );
    field("from    ", `${q.origin} → ${audienceWord(q)}`);
    if (q.leadRecipient) field("lead    ", q.leadRecipient);
    if (q.finding?.path) field("file    ", q.finding.path);
    if (q.blocks) field("blocks  ", q.blocks);
    blank();
    indented("", q.summary, "is-bold");
    push("sample text · this demo holds no private records", "dim");
    indented("  ", q.text);
    if (q.finding) indented("  ", q.finding.detail);
    if (q.approval) {
      blank();
      push("hard to undo · check the target, scope, and revision", "is-bold");
      field("action    ", q.approval.action, "is-bold");
      field("target    ", q.approval.target, "is-bold");
      field("scope     ", q.approval.scope);
      field("revision  ", q.approval.h, "is-bold");
      field("undo      ", q.approval.rollback);
      if (isOpen(q) && q.audience) {
        const on = checked(q);
        blank();
        push(
          `${on ? "[x]" : "[ ]"} I checked the target, scope, and revision (space)`,
          on ? "is-chosen" : "",
          'data-q-check="1"',
        );
        push(
          on
            ? "y approves · n rejects · no answer keeps waiting"
            : "y approves once checked · n rejects · no answer keeps waiting",
          "dim",
        );
      }
    }
    if (choosing(q) && !state.q.answer) {
      const list = choices(q);
      const chosen = choice(q);
      blank();
      push(q.kind === "finding" ? "decide" : "options", "dim");
      list.forEach((label, i) => {
        const text =
          q.kind !== "finding" && label === q.recommendation
            ? `${label} · recommended`
            : label;
        indented(
          i === chosen ? "(•) " : "( ) ",
          text,
          i === chosen ? "is-chosen" : "",
          `data-q-option="${i}"`,
        );
      });
      push(
        q.kind === "finding"
          ? "enter decides"
          : state.q.correcting === q.id
            ? "enter saves the correction · the earlier answer stays in history"
            : "enter saves the answer",
        "dim",
        'data-q-key="Enter"',
      );
    } else if (isOpen(q) && !q.audience) {
      blank();
      push(
        "target unknown · route it with tsk route before anyone answers here",
        "dim",
      );
    } else if (isOpen(q) && q.kind === "question" && !state.q.answer) {
      blank();
      push("enter writes an answer", "dim", 'data-q-key="Enter"');
    }
    if (state.q.answer?.id === q.id) {
      blank();
      rows.push({ input: true });
      push("enter saves · esc cancels", "dim");
    }
    const answer = currentAnswer(q);
    if (answer) {
      blank();
      field("answer  ", answer.text);
      if (q.createdTask) field("task    ", taskLabel(q.createdTask));
      if (answer.action === "create_and_run")
        push(
          "r resends the run request · it never creates another task",
          "dim",
          'data-q-key="r"',
        );
      if (q.kind !== "approval" && state.q.correcting !== q.id)
        push(
          "c corrects · the earlier answer stays in history",
          "dim",
          'data-q-key="c"',
        );
    }
    if (state.q.report[q.id]) {
      blank();
      indented("", state.q.report[q.id], "is-bold");
    }
    blank();
    push("history", "dim");
    if (!q.answers.length) push("asked · no answer yet", "dim");
    q.answers.forEach((a, i) =>
      indented(
        "",
        `${a.actor}${i && q.kind !== "approval" ? " (correction)" : ""} · ${a.text}`,
      ),
    );
    return rows;
  }

  function questionFooterHint(wide) {
    const q = questionById(state.q.page);
    if (state.q.answer) return "enter save · esc cancel";
    if (!q)
      return wide
        ? "enter/→ open · j/k move · z closed · 1 board · ? keys"
        : "enter open · j/k move · z closed · 1 board · ? keys";
    if (isOpen(q) && !q.audience) return "J/K next · t task · esc back";
    if (q.kind === "approval" && isOpen(q))
      return "space check · y approve · n reject · J/K next · esc back";
    if (choosing(q))
      return "j/k choose · enter save · J/K next · t task · esc back";
    if (isOpen(q)) return "enter answer · J/K next · t task · esc back";
    if (q.kind === "approval") return "t task · J/K next · esc back";
    return "c correct · t task · J/K next · esc back";
  }

  // ── question actions (sample only: nothing is stored, sent, or authorized) ──
  function report(q, text) {
    state.q.report[q.id] = text;
  }
  function saveAnswer(q, text, extra = {}) {
    const correction = state.q.correcting === q.id;
    const slot = Math.max(0, questionOrder().indexOf(q.id));
    q.answers.push({ actor: "user", text, ...extra });
    state.q.correcting = null;
    state.q.answer = null;
    state.q.check = null;
    state.message = `Q${q.number} saved · sample`;
    backToList(q, slot);
    return correction;
  }
  function decideFinding(q) {
    const [action, label] = FINDING_ACTIONS[choice(q)];
    const correcting = state.q.correcting === q.id;
    saveAnswer(q, label, { action });
    if (action === "close") {
      report(q, "sample: closed in this demo only · no task");
      return;
    }
    if (q.createdTask) {
      report(
        q,
        `sample: the linked task ${taskLabel(q.createdTask)} stays · no second task${correcting ? " is created by a correction" : ""}`,
      );
      return;
    }
    const source = taskById(q.taskId);
    const task = {
      id: `n${state.nextId++}`,
      number: state.nextNumber++,
      title: q.summary,
      notes: "",
      steps: [],
      status: action === "create_and_run" ? "doing" : "todo",
      project: source?.project ?? null,
      parent: null,
      archived: false,
      createdAt: clock(),
      updatedAt: clock(),
    };
    state.tasks.push(task);
    q.createdTask = task.id;
    report(
      q,
      action === "create_and_run"
        ? `sample: created T${task.number} (doing) in this page only · no run request is sent (the app asks ${q.leadRecipient || "the lead"} when exactly one live agent has that name)`
        : `sample: created T${task.number} (todo) in this page only`,
    );
  }
  function questionEnter() {
    const q = questionById(state.q.page);
    if (!q) return;
    if (state.q.answer) {
      const text = state.q.answer.text.trim();
      if (!text) {
        state.message = "an answer needs text";
        return;
      }
      const correction = saveAnswer(q, text);
      report(
        q,
        `sample: ${correction ? "correction" : "answer"} kept in this page only · nothing is sent (the app hints ${q.origin})`,
      );
      return;
    }
    if (isOpen(q) && !q.audience) {
      state.message = "target unknown · route it with tsk route first";
      return;
    }
    if (choosing(q)) {
      if (q.kind === "finding") decideFinding(q);
      else {
        const correction = saveAnswer(q, choices(q)[choice(q)]);
        report(
          q,
          `sample: ${correction ? "correction" : "answer"} kept in this page only · nothing is sent (the app hints ${q.origin})`,
        );
      }
      return;
    }
    if (q.kind === "question" && (isOpen(q) || state.q.correcting === q.id)) {
      state.q.answer = { id: q.id, text: "" };
    }
  }
  function questionApprove(approved) {
    const q = questionById(state.q.page);
    if (!q || q.kind !== "approval" || !isOpen(q) || !q.audience) return;
    if (approved && !checked(q)) {
      state.message = "check the target, scope, and revision first · space";
      return;
    }
    const slot = Math.max(0, questionOrder().indexOf(q.id));
    q.answers.push({
      actor: "user",
      text: approved ? "approved" : "rejected",
      approved,
    });
    state.q.check = null;
    state.message = `Q${q.number} ${approved ? "approved" : "rejected"} · sample`;
    report(
      q,
      approved
        ? "sample: approved in this demo only · it authorizes nothing"
        : "sample: rejected in this demo only",
    );
    backToList(q, slot);
  }
  function questionCorrect() {
    const q = questionById(state.q.page);
    if (!q || isOpen(q) || q.kind === "approval") return;
    state.q.correcting = q.id;
    if (q.kind === "question" && !q.options.length)
      state.q.answer = { id: q.id, text: "" };
  }
  function questionResend() {
    const q = questionById(state.q.page);
    if (currentAnswer(q)?.action !== "create_and_run") return;
    report(
      q,
      `sample: nothing is sent · the app reads the record back, then asks ${q.leadRecipient || "the lead"}`,
    );
  }
  function openQuestion(id, fromTask = null) {
    if (state.q.page !== id) {
      state.q.check = null;
      state.q.correcting = null;
      state.q.answer = null;
    }
    state.tab = "questions";
    state.q.cursor = id;
    state.q.page = id;
    state.q.returnTask = fromTask;
    clearMarks();
  }
  function stepQuestion(delta) {
    const order = questionOrder();
    const index = order.indexOf(state.q.page);
    if (index < 0) {
      state.message = "this question is not in the list · esc goes back";
      return;
    }
    const next = index + delta;
    if (next < 0) state.message = "first question";
    else if (next >= order.length) state.message = "last question";
    else {
      openQuestion(order[next]);
      state.message = `${next + 1} / ${order.length}`;
    }
  }

  // ── board actions ─────────────────────────────────────────────────────────
  function targetTasks() {
    const marked = state.markMode
      ? [...state.markedIds].map(taskById).filter(Boolean)
      : [];
    if (marked.length) return marked;
    const task = boardFocusOnPage() ? shownTask() : taskById(state.selectedId);
    return task ? [task] : [];
  }
  function clearMarks() {
    state.markMode = false;
    state.markedIds.clear();
  }
  function toggleMark() {
    const id = state.selectedId;
    if (!id || !taskById(id)) return;
    if (state.markedIds.has(id)) state.markedIds.delete(id);
    else state.markedIds.add(id);
  }
  function markCurrent() {
    if (state.selectedId && taskById(state.selectedId))
      state.markedIds.add(state.selectedId);
  }
  function flash(tasks) {
    const last = tasks.at(-1);
    if (!last) return;
    state.flashId = last.id;
    setTimeout(() => {
      if (state.flashId === last.id) {
        state.flashId = null;
        render();
      }
    }, 420);
  }
  function rememberUndo(tasks) {
    state.undo = {
      items: tasks.map((task) => ({
        task: structuredClone(task),
        index: state.tasks.indexOf(task),
      })),
    };
  }
  function setStatus(status) {
    const bulk = state.markMode && state.markedIds.size > 0;
    const tasks = targetTasks();
    let changed;
    let skipped = 0;
    if (status === "doing") {
      changed = tasks.filter((t) => t.status === "todo");
      skipped = tasks.filter((t) => t.status === "done").length;
    } else changed = tasks.filter((t) => t.status !== status);
    if (status === "done" && changed.length) rememberUndo(changed);
    for (const task of changed) {
      task.status = status;
      task.updatedAt = clock();
      task.statusAt = task.updatedAt;
    }
    clearMarks();
    state.pendingDelete = null;
    state.message =
      bulk && skipped
        ? `doing ${changed.length} · skipped ${skipped} done (o reopens them)`
        : "";
    flash(changed);
  }
  function archiveSelected() {
    for (const task of targetTasks()) task.archived = !task.archived;
    clearMarks();
    state.pendingDelete = null;
    state.message = "";
  }
  function deleteSelected() {
    const tasks = state.pendingDelete
      ? state.pendingDelete.map(taskById).filter(Boolean)
      : targetTasks();
    if (!tasks.length) return;
    const bulk = state.pendingDelete
      ? state.pendingDeleteBulk
      : state.markedIds.size > 0;
    const ids = tasks.map((t) => t.id).sort();
    if (
      !state.pendingDelete ||
      state.pendingDelete.slice().sort().join("\n") !== ids.join("\n")
    ) {
      state.pendingDelete = ids;
      state.pendingDeleteBulk = bulk;
      clearMarks();
      state.message = bulk
        ? `press x again to delete ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"}`
        : "press x again to delete";
      return;
    }
    rememberUndo(tasks);
    const idSet = new Set(ids);
    state.tasks = state.tasks.filter((t) => !idSet.has(t.id));
    state.pendingDelete = null;
    state.pendingDeleteBulk = false;
    clearMarks();
    state.trail = state.trail.filter((id) => !idSet.has(id));
    if (state.page && !shownTask()) state.page = false;
    state.message = bulk
      ? `deleted ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"} · u restores`
      : `Deleted "${tasks[0].title}" · u undo`;
  }
  function undo() {
    clearMarks();
    state.pendingDelete = null;
    if (!state.undo) return;
    for (const { task, index } of state.undo.items
      .slice()
      .sort((a, b) => a.index - b.index)) {
      const live = taskById(task.id);
      if (live) Object.assign(live, task);
      else state.tasks.splice(Math.min(index, state.tasks.length), 0, task);
    }
    state.selectedId = state.undo.items[0]?.task.id || state.selectedId;
    state.undo = null;
    state.message = "";
  }
  function setScope(scope) {
    if (hasUnsavedWork()) return;
    clearMarks();
    state.scope = scope;
    state.searchQuery = "";
    state.searchPinned = false;
    state.trail = [];
    state.page = false;
    state.message = "";
    ensureSelection();
  }
  function selectTab(tab) {
    if (tab === state.tab) {
      if (tab === "board") {
        state.page = false;
        state.trail = [];
      }
      return;
    }
    if (tab === "questions" && hasUnsavedWork()) {
      state.message = "save or cancel the edit first";
      return;
    }
    clearMarks();
    state.overlay = null;
    state.tab = tab;
    if (tab === "questions") {
      const order = questionOrder();
      if (!order.includes(state.q.cursor)) state.q.cursor = order[0] ?? null;
      state.q.page = null;
    } else {
      state.page = false;
      state.trail = [];
    }
  }
  function move(delta) {
    state.pendingDelete = null;
    state.message = "";
    const ids = selectableIds();
    if (!ids.length) return;
    let i = ids.indexOf(state.selectedId);
    if (i < 0) i = 0;
    state.selectedId = ids[Math.min(ids.length - 1, Math.max(0, i + delta))];
    state.trail = [];
    state.q.returnQuestion = null;
  }
  function openPage() {
    const row = selectedRow();
    if (row?.kind === "archived") {
      state.archivedOpen = !state.archivedOpen;
      return;
    }
    if (!taskById(state.selectedId)) return;
    clearMarks();
    state.page = true;
    state.trail = [];
    steps.reset();
  }
  function closePage() {
    if (hasUnsavedWork()) {
      state.message = "save or cancel edits first";
      return;
    }
    const backToQuestion =
      state.q.returnQuestion &&
      questionById(state.q.returnQuestion) &&
      state.trail.length <= state.q.returnDepth;
    if (state.trail.length && !backToQuestion) {
      state.trail.pop();
      while (state.trail.length && !taskById(state.trail.at(-1)))
        state.trail.pop();
      steps.reset();
      return;
    }
    if (backToQuestion) {
      const id = state.q.returnQuestion;
      state.trail = [];
      state.q.returnQuestion = null;
      state.page = false;
      openQuestion(id, state.q.returnTask);
      return;
    }
    state.page = false;
    steps.reset();
  }
  // The page or wide detail owns the keys when it is open or focused.
  const boardFocusOnPage = () => state.tab === "board" && state.page;
  function openRelated(id) {
    if (hasUnsavedWork()) {
      state.message = "save or cancel edits before opening another task";
      return;
    }
    if (!taskById(id)) {
      state.message = `${id} not found · link kept`;
      return;
    }
    state.trail.push(id);
    state.page = true;
    steps.reset();
  }
  function stepTask(delta) {
    if (hasUnsavedWork()) {
      state.message = "save or cancel edits before switching tasks";
      return;
    }
    const ids = listTaskIds();
    const current = state.trail.at(-1) ?? state.selectedId;
    const index = ids.indexOf(current);
    if (index < 0) {
      state.message = "this task is not in the list · esc back";
      return;
    }
    const next = index + delta;
    if (next < 0) state.message = "first task in this list";
    else if (next >= ids.length) state.message = "last task in this list";
    else {
      state.selectedId = ids[next];
      state.trail = [];
      state.q.returnQuestion = null;
      steps.reset();
    }
  }

  // The task page ring: parent line, steps, + step, children, questions, links.
  function linksOf(task) {
    const notes = task.notes || "";
    const threadLines = notes
      .split("\n")
      .filter((l) => l.trim().startsWith("Thread:")).length;
    const links = [];
    for (const raw of notes.split("\n")) {
      const line = raw.trim();
      const rest = (prefix) => line.slice(prefix.length).trim().split(/\s+/);
      if (line.startsWith("Linear:")) {
        const [id, url, extra] = rest("Linear:");
        if (id && url && !extra)
          links.push({ label: "issue", text: `${id} ${url}`, url });
      } else if (line.startsWith("PR:") || line.startsWith("CI:")) {
        const prefix = line.startsWith("PR:") ? "PR:" : "CI:";
        const [url, extra] = rest(prefix);
        if (url && !extra)
          links.push({ label: prefix === "PR:" ? "pr" : "ci", text: url, url });
      } else if (line.startsWith("Thread:")) {
        const tokens = rest("Thread:").filter(Boolean);
        if (!tokens.length) continue;
        const ok = threadLines === 1 && tokens.length === 4;
        const reason =
          threadLines > 1
            ? "several Thread lines"
            : tokens.length === 1
              ? "name only"
              : "needs name workspace tab pane";
        links.push({
          label: "thread",
          text: ok
            ? `${tokens[0]} · enter focuses`
            : `${tokens.join(" ")} · cannot verify (${reason})`,
          thread: ok ? tokens[0] : null,
          unverifiable: !ok,
          reason,
        });
      }
    }
    return links;
  }
  const pageQuestions = (task) =>
    (state.questions || [])
      .filter((q) => q.taskId === task.id)
      .sort(
        (a, b) =>
          (a.kind === "finding") - (b.kind === "finding") ||
          a.number - b.number,
      );
  function ring(task) {
    if (state.quickExpanded) return [...task.steps.map((s) => s.id), "add"];
    return [
      "parent",
      ...steps.rows(task).map((s) => s.id),
      "add",
      ...children(task).map((c) => `child:${c.id}`),
      ...pageQuestions(task).map((q) => `q:${q.id}`),
      ...linksOf(task).map((_, i) => `link:${i}`),
    ];
  }
  function moveRing(task, delta) {
    const items = ring(task);
    let i = items.indexOf(steps.selected);
    if (i < 0) {
      // The first move lands on the first step (or + step), as in the app.
      const first = items.findIndex((k) => k !== "parent");
      i = delta > 0 ? first - 1 : first + 1;
    }
    steps.selected = items[(i + delta + items.length) % items.length];
    steps.marked = null;
  }
  function activateRing(task) {
    const key = steps.selected;
    if (!key) return;
    if (key === "add") steps.begin(task);
    else if (key === "parent") {
      if (!task.parent) openParentPicker();
      else openRelated(task.parent);
    } else if (key.startsWith("child:")) openRelated(key.slice(6));
    else if (key.startsWith("q:")) {
      if (hasUnsavedWork()) return;
      state.q.returnQuestion = null;
      openQuestion(key.slice(2), task.id);
    } else if (key.startsWith("link:")) {
      const link = linksOf(task)[Number(key.slice(5))];
      if (!link) return;
      state.message = link.unverifiable
        ? `this Thread line cannot be verified (${link.reason}) · nothing focused`
        : link.thread
          ? `${link.thread} · sample: the demo has no Herdr · nothing focused`
          : link.url;
    } else {
      steps.toggle(task);
      task.updatedAt = clock();
    }
  }

  // ── parent picker ─────────────────────────────────────────────────────────
  function isUnder(candidate, task) {
    let cursor = candidate;
    const seen = new Set();
    while (cursor?.parent && !seen.has(cursor.id)) {
      seen.add(cursor.id);
      if (cursor.parent === task.id) return true;
      cursor = taskById(cursor.parent);
    }
    return false;
  }
  function parentOptions() {
    const task = shownTask();
    if (!task) return [];
    const q = state.parentQ.trim().toLowerCase();
    const options = [{ id: null, label: "no parent" }];
    for (const t of [...state.tasks].sort(byNumber)) {
      if (t.id === task.id) continue;
      const label = `T${t.number} ${t.title}${t.id === task.parent ? " · current" : ""}${t.archived ? " · archived" : ""}${isUnder(t, task) ? " · under this task" : ""}`;
      if (q && !label.toLowerCase().includes(q)) continue;
      options.push({ id: t.id, label, cycle: isUnder(t, task) });
    }
    return options;
  }
  function openParentPicker() {
    if (hasUnsavedWork()) {
      state.message = "save or cancel edits first";
      return;
    }
    state.overlay = "parent";
    state.parentQ = "";
    state.parentI = 0;
  }
  function chooseParent(index) {
    const option = parentOptions()[index];
    const task = shownTask();
    if (!option || !task) return;
    if (option.cycle) {
      state.message = "that task is under this one · choose another";
      return;
    }
    task.parent = option.id;
    task.updatedAt = clock();
    state.overlay = null;
    state.message = option.id
      ? `parent ${taskLabel(option.id)}`
      : "parent cleared";
  }

  // ── editing ───────────────────────────────────────────────────────────────
  function beginEdit(task, field = "title") {
    if (!task) return;
    clearMarks();
    state.edit = {
      field,
      title: task.title,
      notes: task.notes || "",
      scope: task.project ?? null,
    };
  }
  function editStops(task) {
    return [
      "title",
      "notes",
      ...steps.rows(task).map((s) => `step:${s.id}`),
      "step:add",
      "scope",
    ];
  }
  function syncEditInput() {
    const input = document.getElementById("tsk-edit");
    if (input && state.edit) state.edit[state.edit.field] = input.value;
  }
  function moveEdit(task, reverse) {
    syncEditInput();
    const stops = editStops(task);
    let i = stops.indexOf(state.edit.field);
    if (i < 0) i = 0;
    state.edit.field =
      stops[(i + (reverse ? -1 : 1) + stops.length) % stops.length];
    if (state.edit.field.startsWith("step:"))
      steps.selected = state.edit.field.slice(5);
  }
  function saveEdit(task) {
    syncEditInput();
    if (!state.edit) return true;
    const title = state.edit.title.trim();
    if (!title) {
      state.message = "title needed";
      return false;
    }
    task.title = title;
    task.notes = state.edit.notes;
    task.project = state.edit.scope;
    task.updatedAt = clock();
    if (steps.dirty) steps.save(task, true);
    state.edit = null;
    return true;
  }
  function cycleScope(reverse) {
    const names = [null, ...projectNames()];
    const i = names.indexOf(state.edit.scope);
    state.edit.scope =
      names[(i + (reverse ? -1 : 1) + names.length) % names.length];
  }

  // ── quick-add ─────────────────────────────────────────────────────────────
  const fallbackProject = () =>
    state.scope === "all" || state.scope === "desk" ? null : state.scope;
  function openQuickAdd() {
    if (hasUnsavedWork()) return;
    state.overlay = "quick";
    state.draft = "";
    state.refuse = "";
  }
  function saveDraft(stay) {
    const parsed = parseCapture(state.draft, fallbackProject());
    if (!parsed.title) {
      state.refuse = "title needed";
      return false;
    }
    const task = {
      id: `n${state.nextId++}`,
      number: state.nextNumber++,
      title: parsed.title,
      notes: "",
      steps: [],
      status: "todo",
      project: parsed.project,
      parent: null,
      archived: false,
      createdAt: clock(),
      updatedAt: clock(),
    };
    state.tasks.push(task);
    if (inScope(task)) state.selectedId = task.id;
    flash([task]);
    state.draft = "";
    state.refuse = "";
    if (!stay) state.overlay = null;
    return true;
  }
  function expandQuickAdd() {
    const parsed = parseCapture(state.draft, fallbackProject());
    state.capture = {
      id: "capture-draft",
      number: state.nextNumber,
      title: parsed.title,
      notes: "",
      steps: [],
      status: "todo",
      project: parsed.project,
      parent: null,
      createdAt: clock(),
      updatedAt: clock(),
      rawDraft: state.draft,
    };
    state.quickExpanded = true;
    state.refuse = "";
    steps.reset();
    steps.bind(state.capture);
    beginEdit(state.capture, "notes");
  }
  function collapseQuickAdd() {
    syncEditInput();
    const capture = state.capture;
    if (capture && state.edit) {
      const project = /(^|\s)!p(\s|$)/.test(capture.rawDraft)
        ? capture.rawDraft.match(/!p(\s+[^!\s]\S*)?/)[0]
        : "";
      state.draft = [state.edit.title, project].filter(Boolean).join(" ");
    }
    state.quickExpanded = false;
    state.capture = null;
    state.edit = null;
    state.refuse = "";
    steps.reset();
  }
  function saveExpanded() {
    syncEditInput();
    const capture = state.capture;
    if (!capture) return;
    const title = state.edit.title.trim();
    if (!title) {
      state.refuse = "title needed";
      return;
    }
    const task = {
      ...capture,
      id: `n${state.nextId++}`,
      number: state.nextNumber++,
      title,
      notes: state.edit.notes,
      project: state.edit.scope,
      steps: structuredClone(capture.steps),
    };
    delete task.rawDraft;
    state.tasks.push(task);
    state.quickExpanded = false;
    state.capture = null;
    state.edit = null;
    state.overlay = null;
    steps.reset();
    if (inScope(task)) state.selectedId = task.id;
    flash([task]);
  }

  // ── search, palette, help ─────────────────────────────────────────────────
  function updateSearch(value) {
    state.searchQuery = value;
    ensureSelection();
  }
  function paletteCommands() {
    const q = state.paletteQ.trim().toLowerCase();
    const task = taskById(state.selectedId);
    const all = [
      { id: "new", label: "new task", run: openQuickAdd },
      { id: "undo", label: "undo", run: undo },
      {
        id: "drawer",
        label: "done drawer",
        run: () => (state.drawer = !state.drawer),
      },
      {
        id: "help",
        label: "help",
        run: () => {
          state.overlay = "help";
          state.helpQ = "";
        },
      },
      { id: "reset", label: "reset demo", run: resetDemo },
    ];
    if (task)
      all.splice(
        1,
        0,
        { id: "todo", label: "set status: todo", run: () => setStatus("todo") },
        {
          id: "doing",
          label: "set status: doing",
          run: () => setStatus("doing"),
        },
        { id: "done", label: "set status: done", run: () => setStatus("done") },
      );
    // Letters in order, as in the app: `ssd` finds `set status: doing`.
    const fuzzy = (label) => {
      let i = 0;
      for (const c of label) if (c === q[i]) i++;
      return i === q.length;
    };
    return all.filter((c) => !q || fuzzy(c.label));
  }
  function resetDemo() {
    state.tasks = fixture ? structuredClone(fixture.tasks) : seed();
    state.questions = fixture ? (fixture.questions ?? null) : seedQuestions();
    Object.assign(state, {
      tab: "board",
      scope: fixture?.scope ?? launchProject(),
      sort: "number",
      searchQuery: "",
      searchPinned: false,
      selectedId: null,
      message: "",
      drawer: false,
      archivedOpen: false,
      page: false,
      trail: [],
      edit: null,
      undo: null,
    });
    clearMarks();
    state.q = {
      cursor: null,
      page: null,
      options: {},
      check: null,
      answer: null,
      correcting: null,
      report: {},
      returnTask: null,
      returnQuestion: null,
      returnDepth: 0,
      closedOpen: false,
    };
    steps.reset();
  }

  const HELP = [
    ["navigation", "↑↓ / jk", "move", "select"],
    ["navigation", "enter", "open task (focus the detail at 110+)", "detail"],
    ["navigation", "→/l · ←/h", "detail or list at 110+ columns", "wide"],
    ["navigation", "J / K", "next / previous task or question", "step"],
    ["navigation", "shift+M", "multi-select", "select multiple"],
    ["navigation", "space", "toggle mark (multi-select)", "select multiple"],
    ["navigation", "shift+↑↓", "mark and move (multi-select)", "select"],
    ["task actions", "s", "doing (ctrl+s in the app)", "status"],
    ["task actions", "o", "todo (ctrl+o in the app)", "status reopen"],
    ["task actions", "d", "done (ctrl+d in the app)", "status finish"],
    ["task actions", "x", "delete, press twice", "remove"],
    ["task actions", "u", "undo", "restore"],
    ["task actions", "f", "archive / restore", "file hide"],
    ["create & edit", "+", "quick-add", "capture new task"],
    ["create & edit", "e", "edit title (task page)", "rename"],
    ["create & edit", "a", "add a step (task page)", "checklist"],
    ["create & edit", "alt+↑ / alt+↓", "move selected step", "reorder"],
    ["create & edit", "P / U", "choose / clear the parent", "relation"],
    ["views & find", "1", "board tab", "switch"],
    ["views & find", "2", "questions tab", "agent answer approve"],
    ["views & find", "p", "scope: all, desk, or a project", "project"],
    ["views & find", "S", "sort by number / by project", "order"],
    ["views & find", "z / D", "done drawer", "completed"],
    ["views & find", "g", "fold / unfold archived", "drawer"],
    ["views & find", "/", "search board", "find filter"],
    ["views & find", ":", "command palette", "find actions"],
    ["questions", "enter", "open · save the chosen answer", "answer"],
    ["questions", "j / k", "choose an option", "answer"],
    ["questions", "space", "check an approval's terms", "approve"],
    ["questions", "y / n", "approve once checked / reject", "approve"],
    ["questions", "c", "correct an answer", "answer"],
    ["questions", "t", "open the question's task", "task"],
    ["questions", "r", "resend a lead request", "finding"],
    ["app controls", "?", "help", "shortcuts keys"],
    ["app controls", "esc", "clear / close / back", "cancel"],
  ];

  // ── rendering ─────────────────────────────────────────────────────────────
  function renderTabs(width) {
    const count = questionsLoaded() && forYou() ? ` ·${forYou()}` : "";
    const scope = `${state.scope} ▾`;
    const sort = `sort ${state.sort} ▾`;
    const variants = [
      ["questions", sort],
      ["questions", sort.replace("sort ", "")],
      ["", sort.replace("sort ", "")],
    ];
    const sep = "  ·  ";
    const onBoard = state.tab === "board";
    let chosen = variants[variants.length - 1];
    for (const v of variants) {
      const label = `2${v[0] ? ` ${v[0]}` : ""}${count}`;
      const parts = onBoard
        ? ["1 board", label, scope, v[1]]
        : ["1 board", label];
      if ((" " + parts.join(sep)).length <= width) {
        chosen = v;
        break;
      }
    }
    const qLabel = `2${chosen[0] ? ` ${chosen[0]}` : ""}${count}`;
    const tab = (name, text) =>
      `<span class="tsk-tab-group ${state.tab === name ? "is-on" : ""}"><button type="button" class="tsk-tab ${state.tab === name ? "is-on" : ""}" data-tab="${name}">${esc(text)}</button></span>`;
    const dot = `<span class="dim">${sep}</span>`;
    let html = ` ${tab("board", "1 board")}${dot}${tab("questions", qLabel)}`;
    if (onBoard)
      html += `${dot}<button type="button" class="tsk-selector dim" data-scope="1">${esc(scope)}</button>${dot}<button type="button" class="tsk-selector dim" data-sort="1">${esc(chosen[1])}</button>`;
    return `<div class="tsk-tabs">${html}</div>`;
  }

  // `steps 1/3 · 33%`, rounded down like the board; no steps keeps `steps 0/0`.
  function stepsHeading(rows) {
    const done = rows.filter((s) => s.done).length;
    if (!rows.length) return "steps 0/0";
    return `steps ${done}/${rows.length} · ${Math.floor((done * 100) / rows.length)}%`;
  }

  function renderTaskRow(row, width) {
    const task = row.task;
    const selected = task.id === state.selectedId;
    const marked = state.markedIds.has(task.id);
    const prefix = `${selected ? "▸" : " "}${marked ? "▪" : " "} `;
    const id = `T${task.number}`;
    const kids = children(task);
    const badge = kids.length
      ? `⊂${kids.filter((c) => c.status === "done").length}/${kids.length}`
      : "";
    const alert = taskMarks(task);
    const tailWidth = alert.length + badge.length + (alert && badge ? 1 : 0);
    const room = Math.max(
      8,
      width -
        2 -
        prefix.length -
        id.length -
        1 -
        (tailWidth ? tailWidth + 1 : 0),
    );
    const title = wrapText(task.title, room)
      .map((line) => `<span class="tsk-title-line">${esc(line)}</span>`)
      .join("");
    const tail = tailWidth
      ? `<span class="tsk-row-tail">${alert ? `<b class="tsk-alert">${esc(alert)}</b>` : ""}${alert && badge ? " " : ""}${badge ? `<span class="dim">${esc(badge)}</span>` : ""}</span>`
      : "";
    return `<button type="button" class="tsk-row ${row.dim ? "dim" : ""} ${selected ? "is-sel" : ""} ${task.id === state.flashId ? "is-flash" : ""}" data-task="${esc(task.id)}"><span class="tsk-row-main"><span class="tsk-row-prefix">${prefix}<span class="tsk-task-id" data-copy-task="${esc(task.id)}" title="copy ${id}">${id}</span> </span><span class="tsk-row-title">${title}</span>${tail}</span></button>`;
  }

  function renderBoardList(width) {
    const rows = buildRows();
    ensureSelection(rows);
    const body = rows
      .map((row) => {
        if (row.kind === "section" && row.collapsed)
          return `<button type="button" class="tsk-sec" data-drawer="1"><span class="sec">${esc(row.label)}</span><span class="rule" aria-hidden="true"></span><span class="count">${row.count} · z to open</span></button>`;
        if (row.kind === "section")
          return `<div class="tsk-sec"><span class="sec">${esc(row.label)}</span><span class="rule" aria-hidden="true"></span><span class="count">${row.count}</span></div>`;
        if (row.kind === "sub")
          return `<div class="tsk-sub-label dim">   ${esc(row.label)}</div>`;
        if (row.kind === "archived") {
          const selected = row.id === state.selectedId;
          return `<button type="button" class="tsk-group" data-archived-header="1"><span class="dim"> ${state.archivedOpen ? "▾" : "▸"}</span> <span class="sec">${selected ? "<strong>archived</strong>" : "archived"}</span><span class="rule" aria-hidden="true"></span><span class="count dim">${row.count}</span></button>`;
        }
        return renderTaskRow(row, width);
      })
      .join("");
    const line = waitingLine();
    const waitingRow = line
      ? `<button type="button" class="tsk-waiting" data-waiting="1">${wrapText(
          ` ${line}`,
          width - 1,
        )
          .map((l) => `<span>${esc(l)}</span>`)
          .join("")}</button>`
      : "";
    const empty = state.searchQuery.trim()
      ? `no tasks match &quot;${esc(state.searchQuery.trim())}&quot;`
      : "no open tasks here — p rescope or + add";
    return `${renderTabs(width)}${waitingRow}<div class="tsk-list">${body || `<div class="dim">    ${empty}</div>`}</div>`;
  }

  function relationRow(key, lead, text, { dim = false, width }) {
    const selected = steps.selected === key && !state.edit;
    const lines = wrapText(text, Math.max(8, width - 6 - lead.length));
    return `<div class="tsk-relation ${dim && !selected ? "dim" : ""}" ${key ? `data-ring="${esc(key)}"` : ""} ${key ? `aria-selected="${selected}"` : ""}><span class="tsk-step-glyph">${selected ? "▸ " : "  "}</span><span class="tsk-relation-text">${lines
      .map(
        (line, i) =>
          `<span>${esc((i ? " ".repeat(lead.length) : lead) + line)}</span>`,
      )
      .join("")}</span></div>`;
  }

  function renderTaskPage(task, { embedded, focused }) {
    const narrow = !isWide();
    const width = detailColumns();
    if (!task)
      return `<div class="tsk-task-column tsk-surface"><div class="tsk-task-header dim"><span class="sec">no task</span></div><div class="tsk-task-rule" aria-hidden="true"></div><div class="tsk-task-surface"><div class="dim">  select a task to see it here</div></div></div>`;
    steps.bind(task);
    const edit = state.edit;
    const field = edit?.field;
    const scopeName = edit ? (edit.scope ?? "desk") : projectName(task);
    const stateSlot = steps.editor
      ? "editing step"
      : steps.dirty
        ? "unsaved"
        : edit
          ? `editing ${field.startsWith("step:") ? "steps" : field}`
          : narrow
            ? task.status
            : `${task.status} · ${scopeName}`;
    const glyph = GLYPH[task.status] || "○";
    const titleText = edit ? edit.title : task.title;
    let header;
    if (field === "title") {
      header = `<div class="tsk-task-header is-bold"><span class="glyph">${glyph}</span> <span class="tsk-task-id">T${task.number}</span> <input class="tsk-field" id="tsk-edit" value="${esc(edit.title)}" /><span class="tsk-state-slot">${esc(stateSlot)}</span></div><div class="tsk-task-rule" aria-hidden="true"></div>`;
    } else if (narrow) {
      const room = terminalColumns() - 11 - stateSlot.length;
      const titleRows = wrapText(titleText, room);
      const status =
        titleRows[0].length + 8 + stateSlot.length <= terminalColumns() - 2
          ? stateSlot
          : "";
      header = `<div class="tsk-task-header tsk-narrow-header"><span class="tsk-page-prefix">${glyph} <span class="tsk-task-id" data-copy-task="${esc(task.id)}">T${task.number}</span> </span><span class="tsk-page-title">${titleRows.map((l) => `<span>${esc(l)}</span>`).join("")}</span><span class="tsk-state-slot">${esc(status)}</span></div><div class="tsk-task-rule"></div>`;
    } else {
      header = `<div class="tsk-task-header ${focused ? "is-bold" : "dim"}"><span class="glyph">${glyph}</span> <span class="tsk-task-id" data-copy-task="${esc(task.id)}" title="copy T${task.number}">T${task.number}</span> <span class="sec">${esc(titleText)}</span><span class="tsk-state-slot">${esc(stateSlot)}</span></div><div class="tsk-task-rule" aria-hidden="true"></div>`;
    }
    const notesText = edit ? edit.notes : task.notes;
    const notes =
      field === "notes"
        ? `<textarea class="tsk-field tsk-notes" id="tsk-edit">${esc(edit.notes)}</textarea>`
        : `<div class="tsk-page-notes">${wrapText(
            notesText || "no notes yet",
            width - 6,
          )
            .map((l) => `<span>${esc(l) || " "}</span>`)
            .join("")}</div>`;
    const relations = [];
    if (!state.quickExpanded) {
      const parent = task.parent ? taskById(task.parent) : null;
      const parentText = !task.parent
        ? "none · P to set"
        : parent
          ? `↑ T${parent.number} ${parent.title} · ${parent.archived ? "archived" : parent.status}`
          : `↑ ${task.parent} not found · link kept`;
      relations.push(
        relationRow("parent", "parent  ", parentText, {
          dim: !parent,
          width,
        }),
      );
    }
    const stepRows = steps.rows(task);
    // wrapText counts a line's trailing space, so 8 here matches the app's step rows.
    const stepWidth = Math.max(8, width - 8);
    const editor = `<textarea id="tsk-step-edit" class="tsk-field" aria-label="Step text" rows="${wrapText(steps.editor?.text ?? "", stepWidth).length}">${esc(steps.editor?.text ?? "")}</textarea><span class="tsk-step-refusal">${esc(steps.refusal)}</span>`;
    const stepSelected = (id) =>
      edit ? field === `step:${id}` : steps.selected === id;
    const stepList = `<div class="tsk-steps"><div class="tsk-steps-heading dim">${stepsHeading(stepRows)}</div>${stepRows
      .map(
        (step) =>
          `<div class="tsk-step" data-step="${esc(step.id)}" role="option" aria-selected="${stepSelected(step.id)}"><span class="tsk-step-glyph">${stepSelected(step.id) ? "▸ " : "  "}${steps.marked === step.id ? "✗" : step.done ? "✓" : "▪"} </span>${
            steps.editor?.id === step.id
              ? editor
              : `<span class="tsk-step-text">${wrapText(step.text, stepWidth)
                  .map((l) => `<span>${esc(l)}</span>`)
                  .join("")}</span>`
          }</div>${steps.editor && !steps.editor.id && steps.editor.after === step.id ? `<div class="tsk-step-new">${editor}</div>` : ""}`,
      )
      .join(
        "",
      )}${steps.editor && !steps.editor.id && !steps.editor.after ? `<div class="tsk-step-new">${editor}</div>` : `<button type="button" class="tsk-step-add dim" data-step-add="1" aria-selected="${stepSelected("add")}">${stepSelected("add") ? "▸" : " "}  + step</button>`}</div>`;
    const after = [];
    if (!state.quickExpanded) {
      const kids = children(task);
      if (kids.length) {
        const done = kids.filter((c) => c.status === "done").length;
        after.push(
          relationRow(null, "", `children ${done}/${kids.length} done`, {
            dim: true,
            width,
          }),
        );
        for (const c of kids)
          after.push(
            relationRow(
              `child:${c.id}`,
              `${GLYPH[c.status]} `,
              `T${c.number} ${c.title}${c.archived ? " · archived" : ""}`,
              { dim: c.archived, width },
            ),
          );
        if (done === kids.length && task.status !== "done")
          after.push(
            relationRow(null, "", "all children done · d completes this task", {
              dim: true,
              width,
            }),
          );
      }
      const records = pageQuestions(task);
      const asks = records.filter((q) => q.kind !== "finding");
      const findings = records.filter((q) => q.kind === "finding");
      if (asks.length) {
        after.push(
          relationRow(
            null,
            "",
            `questions ${asks.filter(isOpen).length} open`,
            { dim: true, width },
          ),
        );
        for (const q of asks)
          after.push(
            relationRow(
              `q:${q.id}`,
              q.kind === "approval" ? "! " : "? ",
              `Q${q.number} ${q.summary} · ${stateLabel(q)}`,
              { dim: !isOpen(q), width },
            ),
          );
      }
      if (findings.length) {
        after.push(relationRow(null, "", "findings", { dim: true, width }));
        for (const q of findings)
          after.push(
            relationRow(
              `q:${q.id}`,
              "◇ ",
              `Q${q.number} ${q.finding?.kind || "issue"} · ${q.summary} · ${stateLabel(q)}`,
              { dim: !isOpen(q), width },
            ),
          );
      }
      const links = linksOf(task);
      if (links.length) {
        after.push(relationRow(null, "", "links", { dim: true, width }));
        links.forEach((link, i) =>
          after.push(
            relationRow(`link:${i}`, link.label.padEnd(7), link.text, {
              dim: link.unverifiable,
              width,
            }),
          ),
        );
      }
    }
    const scopeField =
      field === "scope"
        ? `<span class="tsk-meta-selected" data-page-field="scope">${esc(scopeName)}</span>`
        : esc(scopeName);
    const meta = [
      narrow || edit ? scopeField : "",
      `created ${esc(age(task.createdAt))} ago`,
      `updated ${esc(age(task.updatedAt))} ago`,
    ]
      .filter(Boolean)
      .join(" · ");
    const editField = field?.startsWith("step:") ? "steps" : field || "";
    const editTarget = field?.startsWith("step:") ? field.slice(5) : "";
    return `<div class="tsk-task-column tsk-surface ${narrow ? "is-narrow" : ""}" aria-label="T${task.number} task column" data-status="${esc(task.status)}" data-edit-state="${steps.editor ? "editing" : steps.dirty ? "unsaved" : edit ? "editing" : "view"}" data-edit-field="${esc(editField)}" data-edit-target="${esc(editTarget)}">${header}<div class="tsk-task-surface tsk-page">${notes}${relations.length ? `<div class="tsk-relations">${relations.join("")}</div>` : ""}${stepList}${after.length ? `<div class="tsk-relations tsk-after-steps">${after.join("")}</div>` : ""}</div><div class="tsk-page-meta dim">${meta}</div></div>`;
  }

  function renderQuestionList(width, wide) {
    if (!questionsLoaded())
      return wrapText(
        "  questions live on an Agent board · this sample board has no Agent questions, findings, or Threads",
        width,
      )
        .map((l) => `<div class="dim">${esc(l)}</div>`)
        .join("");
    const groups = grouped();
    if (!groups.length) return `<div class="dim">  no questions</div>`;
    return groups
      .map(([key, title, members]) => {
        const rows = members
          .map((q) => {
            const selected = state.q.cursor === q.id;
            const meta =
              key === "closed"
                ? ` · ${stateLabel(q)}`
                : key === "unrouted"
                  ? " · target unknown"
                  : "";
            const lead = `${selected ? " ▸ " : "   "}Q${q.number} `;
            const lines = wrapText(
              `${q.summary}${meta}`,
              Math.max(8, width - lead.length),
            );
            const bold = selected && (!state.q.page || !wide);
            return `<button type="button" class="tsk-q-row ${bold ? "is-bold" : ""}" data-question="${esc(q.id)}">${lines
              .map(
                (l, i) =>
                  `<span>${esc((i ? " ".repeat(lead.length) : lead) + l)}</span>`,
              )
              .join("")}</button>`;
          })
          .join("");
        if (key === "closed")
          return `<div class="tsk-q-group is-bold" data-q-key="z"> ${esc(title)} ${members.length} · z ${state.q.closedOpen ? "hides" : "shows"}</div>${state.q.closedOpen ? rows : ""}`;
        return `<div class="tsk-q-group ${key === "unrouted" ? "dim" : "is-bold"}"> ${esc(title)} ${members.length}</div>${rows}`;
      })
      .join("");
  }

  function renderQuestionPage(q, width) {
    return `<div class="tsk-q-page" data-q-page="${esc(q.id)}">${questionPageRows(
      q,
      width,
    )
      .map((row) => {
        if (row.input)
          return `<div class="tsk-q-line is-chosen"><span>answer  </span><input class="tsk-field" id="tsk-answer" value="${esc(state.q.answer.text)}" autocomplete="off" aria-label="Answer" /></div>`;
        // A field paints its label dim and only its value in the row's style.
        if (row.label)
          return `<div class="tsk-q-line" ${row.attrs}><span class="dim">${esc(row.text.slice(0, row.label))}</span><span class="${row.cls}">${esc(row.text.slice(row.label))}</span></div>`;
        return `<div class="tsk-q-line ${row.cls}" ${row.attrs}>${esc(row.text) || " "}</div>`;
      })
      .join("")}</div>`;
  }

  function renderQuestionsTab() {
    const wide = isWide();
    const width = wide ? listColumns() : terminalColumns();
    const list = `<div class="tsk-list tsk-q-list">${renderQuestionList(width - 1, wide)}</div>`;
    const shown = questionById(state.q.page ?? (wide ? state.q.cursor : null));
    if (wide) {
      const detail = shown
        ? renderQuestionPage(shown, detailColumns() - 2)
        : `<div class="dim">  pick a question on the left</div>`;
      return `${renderTabs(terminalColumns())}<div class="tsk-wide-split is-split" style="grid-template-columns:${listColumns()}ch 2ch minmax(0,1fr)"><div class="tsk-board-surface tsk-surface">${list}</div><div class="tsk-rule-column dim" aria-hidden="true"></div><div class="tsk-surface tsk-q-detail ${state.q.page ? "is-focused" : ""}">${detail}</div></div>`;
    }
    return `${renderTabs(width)}${shown ? `<div class="tsk-list">${renderQuestionPage(shown, width - 2)}</div>` : list}`;
  }

  function contextLine() {
    if (state.tab === "questions") {
      if (!questionsLoaded()) return "questions · no Agent board here";
      return `questions · ${forYou()} waiting on you · ${(state.questions || []).length} total · sample, nothing is sent`;
    }
    const base =
      state.searchPinned && state.searchQuery.trim()
        ? `${state.scope} · /${state.searchQuery.trim()}`
        : state.scope;
    if (state.markMode)
      return state.markedIds.size
        ? `multi-select · ${state.markedIds.size} selected · esc clears`
        : "multi-select · space/click marks · esc exits";
    return base;
  }

  function verbButtons(items) {
    return items
      .map(
        (v) =>
          `<button type="button" class="tsk-verb" data-verb="${esc(v.id)}">${esc(v.label)}</button>`,
      )
      .join("<span> · </span>");
  }
  // The key a questions footer entry names; the `j/k` pair only names keys.
  function questionFooterKey(token) {
    if (token === "enter" || token === "enter/→") return "Enter";
    if (token === "esc") return "Escape";
    if (token === "space") return " ";
    if (token === "J/K") return "J";
    return [...token].length === 1 ? token : null;
  }
  function questionFooterButtons(hint) {
    return hint
      .split(" · ")
      .map((entry) => {
        const key = questionFooterKey(entry.split(" ")[0]);
        return key
          ? `<button type="button" class="tsk-verb" data-q-key="${esc(key)}">${esc(entry)}</button>`
          : `<span>${esc(entry)}</span>`;
      })
      .join("<span> · </span>");
  }
  function hintBar(items) {
    return items
      .map(
        (item) =>
          `<button type="button" class="tsk-verb" data-hint="${esc(item.id)}">${esc(item.label)}</button>`,
      )
      .join("<span> · </span>");
  }

  function renderFooter() {
    const wide = isWide();
    let hint = "";
    let verbs;
    if (state.tab === "questions") {
      verbs = questionFooterButtons(questionFooterHint(wide));
    } else if (state.edit || steps.editor || steps.dirty) {
      verbs = "shift+enter save · tab next field · esc cancel";
    } else if (state.page) {
      const task = shownTask();
      verbs = verbButtons([
        { id: "edit", label: "e edit" },
        task?.status === "todo"
          ? { id: "doing", label: "s doing" }
          : { id: "todo", label: "o todo" },
        task?.status === "done"
          ? { id: "undo", label: "u undo" }
          : { id: "done", label: "d done" },
        { id: "close", label: wide ? "esc list" : "esc close" },
      ]);
      if (wide) hint = "← list";
    } else {
      const task = taskById(state.selectedId);
      const items = [{ id: "open", label: "enter open" }];
      if (task) {
        if (task.status === "todo")
          items.push({ id: "doing", label: "s doing" });
        else items.push({ id: "todo", label: "o todo" });
        if (task.status !== "done") items.push({ id: "done", label: "d done" });
      }
      items.push(
        { id: "capture", label: "+ add" },
        { id: "help", label: "? help" },
      );
      verbs = verbButtons(items);
      if (wide && task) hint = "→ detail";
    }
    let body;
    if (state.overlay === "quick" && !state.quickExpanded) {
      body = `<div class="tsk-input-row"><span class="tsk-prompt">+</span><input class="tsk-field" id="tsk-add" value="${esc(state.draft)}" placeholder="title  ·  !p project" autocomplete="off" /><span class="cursor">█</span></div><div class="foot dim tsk-verbs">${
        state.refuse
          ? esc(state.refuse)
          : hintBar([
              { id: "save", label: "enter save" },
              { id: "details", label: "tab details" },
              { id: "close", label: "esc close" },
            ])
      }</div>`;
    } else if (state.quickExpanded) {
      body = `<div class="tsk-status-row"><span class="foot">new task · sample, saved in this page only</span></div><div class="foot dim tsk-verbs">${
        state.refuse
          ? esc(state.refuse)
          : hintBar([
              { id: "save-expanded", label: "shift+enter save" },
              { id: "next-field", label: "tab next field" },
              { id: "collapse", label: "esc one-line draft" },
            ])
      }</div>`;
    } else if (state.overlay === "search") {
      body = `<div class="tsk-input-row"><span class="tsk-prompt">/</span><input class="tsk-field" id="tsk-search" value="${esc(state.searchQuery)}" placeholder="search tasks" autocomplete="off" /><span class="cursor">█</span></div><div class="foot dim tsk-verbs">${hintBar(
        [
          { id: "pin", label: "enter pin" },
          { id: "clear", label: "esc clear" },
        ],
      )}</div>`;
    } else {
      const context = state.message || contextLine();
      body = `<div class="tsk-status-row"><button type="button" class="tsk-done-count foot" data-drawer="1">${esc(context)}</button><span class="foot dim tsk-stage-hint">${esc(hint)}</span></div><div class="foot dim tsk-verbs">${verbs}</div>${state.copyNotice ? `<div class="foot dim">${esc(state.copyNotice)}</div>` : ""}`;
    }
    return `<div class="tsk-foot"><div class="foot-rule" aria-hidden="true"></div>${body}</div>`;
  }

  function renderHelp() {
    const query = state.helpQ.trim().toLowerCase();
    const visible = HELP.filter(
      (row) => !query || row.join(" ").toLowerCase().includes(query),
    );
    let previous = "";
    const list = visible
      .map((row) => {
        const heading =
          row[0] === previous
            ? ""
            : `<div class="tsk-help-group">${esc(row[0].toUpperCase())}</div>`;
        previous = row[0];
        return `${heading}<div class="tsk-help-row"><span>${esc(row[1])}</span><span>${esc(row[2])}</span></div>`;
      })
      .join("");
    return `<div class="tsk-box tsk-help" role="dialog" aria-label="help"><div class="tsk-box-top"><span class="tsk-box-title">help</span><button type="button" class="tsk-box-close" data-close="1" aria-label="close">[x]</button></div><div class="tsk-help-search"><span>/ </span><span>${state.helpQ ? esc(state.helpQ) : '<span class="dim">search keys or actions…</span>'}</span><span class="cursor">█</span></div><div class="tsk-help-divider" aria-hidden="true"></div><div class="tsk-box-body tsk-help-body">${list || '<div class="dim">no shortcuts match</div>'}</div><div class="tsk-box-foot">type search · ↑/↓ scroll · esc clear/close · the demo takes bare s o d for ctrl chords</div></div>`;
  }
  function renderPalette() {
    const cmds = paletteCommands();
    if (state.paletteI >= cmds.length)
      state.paletteI = Math.max(0, cmds.length - 1);
    const list = cmds
      .map(
        (c, i) =>
          `<div class="tsk-pal-row ${i === state.paletteI ? "sel-text" : ""}" data-cmd="${esc(c.id)}">${i === state.paletteI ? "▸" : " "} ${esc(c.label)}</div>`,
      )
      .join("");
    return `<div class="tsk-box tsk-palette" role="dialog" aria-label="command"><div class="tsk-box-top"><span class="tsk-box-title">command</span><button type="button" class="tsk-box-close" data-close="1" aria-label="close">[x]</button></div><div class="tsk-box-body">${list || `<div class="dim">  no matches</div>`}</div><div class="tsk-box-foot">↑/↓ move · enter run · esc close · type to filter</div></div><div class="tsk-input-row"><span class="tsk-prompt">:</span><span class="tsk-draft">${esc(state.paletteQ)}</span><span class="cursor">█</span></div>`;
  }
  function renderScopePicker() {
    const opts = scopeOptions();
    const list = opts
      .map(
        (name, i) =>
          `<div class="tsk-pal-row" data-pick="${esc(name)}"><span class="${i === state.pickerI ? "sel-text" : ""}">${i === state.pickerI ? "▸" : " "} ${esc(name)}${name === state.scope ? " · current" : ""}</span></div>`,
      )
      .join("");
    return `<div class="tsk-box tsk-palette" role="dialog" aria-label="scope"><div class="tsk-box-top"><span class="tsk-box-title">scope</span><button type="button" class="tsk-box-close" data-close="1" aria-label="close">[x]</button></div><div class="tsk-box-body">${list}</div><div class="tsk-box-foot">↑/↓ move · enter open · esc close</div></div>`;
  }
  function renderParentPicker() {
    const opts = parentOptions();
    if (state.parentI >= opts.length)
      state.parentI = Math.max(0, opts.length - 1);
    const list = opts
      .map(
        (o, i) =>
          `<div class="tsk-pal-row ${o.cycle ? "dim" : ""}" data-parent-option="${i}"><span class="${i === state.parentI ? "sel-text" : ""}">${i === state.parentI ? "▸" : " "} ${esc(o.label)}</span></div>`,
      )
      .join("");
    return `<div class="tsk-box tsk-palette" role="dialog" aria-label="parent"><div class="tsk-box-top"><span class="tsk-box-title">parent</span><button type="button" class="tsk-box-close" data-close="1" aria-label="close">[x]</button></div><div class="tsk-help-search"><span>/ </span><span>${state.parentQ ? esc(state.parentQ) : '<span class="dim">type a number or title…</span>'}</span><span class="cursor">█</span></div><div class="tsk-box-body">${list}</div><div class="tsk-box-foot">↑/↓ move · enter save · esc close</div></div>`;
  }

  function render() {
    renderColumns = terminalColumns();
    try {
      const wide = isWide();
      let html;
      if (state.tab === "questions") {
        html = renderQuestionsTab() + renderFooter();
      } else if (state.quickExpanded) {
        html = `<div class="tsk-single-task">${renderTaskPage(state.capture, { focused: true })}</div>${renderFooter()}`;
      } else if (wide) {
        const list = renderBoardList(listColumns());
        html = `<div class="tsk-wide-split is-split" style="grid-template-columns:${listColumns()}ch 2ch minmax(0,1fr)"><div class="tsk-board-surface tsk-surface">${list}</div><div class="tsk-rule-column dim" aria-hidden="true"></div>${renderTaskPage(shownTask(), { focused: state.page })}</div>${renderFooter()}`;
      } else if (state.page) {
        html = `<div class="tsk-single-task">${renderTaskPage(shownTask(), { focused: true })}</div>${renderFooter()}`;
      } else {
        html = renderBoardList(terminalColumns()) + renderFooter();
      }
      if (state.overlay === "help") html += renderHelp();
      if (state.overlay === "palette") html += renderPalette();
      if (state.overlay === "picker") html += renderScopePicker();
      if (state.overlay === "parent") html += renderParentPicker();
      const oldScroll = root.querySelector(".tsk-task-surface")?.scrollTop ?? 0;
      const keepKeys =
        document.activeElement === frame ||
        frame.contains(document.activeElement);
      root.innerHTML = html;
      const content = root.querySelector(".tsk-task-surface");
      if (content) content.scrollTop = oldScroll;
      const focusInput = (input, onInput) => {
        input.addEventListener("input", () => onInput(input.value));
        input.focus({ preventScroll: true });
        const end = input.value.length;
        input.setSelectionRange?.(end, end);
      };
      const stepInput = root.querySelector("#tsk-step-edit");
      const answer = root.querySelector("#tsk-answer");
      const add = root.querySelector("#tsk-add");
      const edit = root.querySelector("#tsk-edit");
      const search = root.querySelector("#tsk-search");
      if (stepInput) {
        focusInput(stepInput, (value) => {
          steps.editor.text = value;
          steps.refusal = "";
          root.querySelector(".tsk-step-refusal").textContent = "";
        });
        if (keepKeys) stepInput.scrollIntoView({ block: "nearest" });
      } else if (answer)
        focusInput(answer, (value) => (state.q.answer.text = value));
      else if (add)
        focusInput(add, (value) => {
          state.draft = value;
          state.refuse = "";
        });
      else if (edit)
        focusInput(edit, (value) => (state.edit[state.edit.field] = value));
      else if (search)
        focusInput(search, (value) => {
          updateSearch(value);
          render();
        });
      else if (keepKeys) frame.focus({ preventScroll: true });
      // Keep the cursor row in view inside the board, only while the board has the keys,
      // so a resize never scrolls the landing page.
      if (keepKeys)
        root
          .querySelector(
            '[data-ring][aria-selected="true"], [data-q-check], .tsk-q-line.is-chosen',
          )
          ?.scrollIntoView({ block: "nearest" });
      frame.classList.toggle(
        "is-focused",
        document.activeElement === frame ||
          frame.contains(document.activeElement),
      );
    } finally {
      renderColumns = null;
    }
  }

  // ── keyboard ──────────────────────────────────────────────────────────────
  const plain = (e) => !e.ctrlKey && !e.metaKey && !e.altKey;
  const STATUS_KEYS = ["s", "o", "d", "x", "u", "f"];

  function handleTextField(e) {
    const el = e.target;
    if (el.id === "tsk-search") {
      if (e.key === "Escape") {
        e.preventDefault();
        updateSearch("");
        state.searchPinned = false;
        state.overlay = null;
      } else if (e.key === "Enter" && plain(e)) {
        e.preventDefault();
        state.searchPinned = Boolean(state.searchQuery.trim());
        if (!state.searchPinned) updateSearch("");
        state.overlay = null;
      } else return true;
      render();
      frame.focus({ preventScroll: true });
      return true;
    }
    if (el.id === "tsk-answer") {
      if (e.key === "Escape") {
        e.preventDefault();
        state.q.answer = null;
        if (state.q.correcting) state.q.correcting = null;
      } else if (e.key === "Enter" && plain(e)) {
        e.preventDefault();
        state.q.answer.text = el.value;
        questionEnter();
      } else return true;
      render();
      return true;
    }
    if (el.id === "tsk-add") {
      if (e.key === "Escape") {
        e.preventDefault();
        state.overlay = null;
        state.refuse = "";
      } else if (e.key === "Enter" && plain(e)) {
        e.preventDefault();
        state.draft = el.value;
        saveDraft(e.shiftKey);
      } else if (e.key === "Tab") {
        e.preventDefault();
        state.draft = el.value;
        expandQuickAdd();
      } else return true;
      render();
      if (!state.overlay) frame.focus({ preventScroll: true });
      return true;
    }
    if (el.id === "tsk-edit") {
      const task = shownTask();
      if (e.key === "Tab") {
        e.preventDefault();
        moveEdit(task, e.shiftKey);
      } else if (e.key === "Escape") {
        e.preventDefault();
        if (state.quickExpanded) collapseQuickAdd();
        else {
          state.edit = null;
          steps.cancel();
          steps.reset();
        }
      } else if (e.key === "Enter" && (e.shiftKey || e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        if (state.quickExpanded) saveExpanded();
        else saveEdit(task);
      } else if (e.key === "Enter" && state.edit.field === "title") {
        e.preventDefault();
        moveEdit(task, false);
      } else return true;
      render();
      if (!state.edit) frame.focus({ preventScroll: true });
      return true;
    }
    if (el.id === "tsk-step-edit") {
      const task = shownTask();
      if (e.key === "Escape") {
        e.preventDefault();
        steps.cancel();
      } else if (e.key === "Enter" && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        const persists = !steps.editor.id || e.shiftKey;
        if (steps.save(task, e.shiftKey && !state.edit) && persists)
          task.updatedAt = clock();
      } else return true;
      render();
      if (!steps.editor) frame.focus({ preventScroll: true });
      return true;
    }
    return false;
  }

  function handleOverlay(e) {
    const o = state.overlay;
    if (o === "help") {
      e.preventDefault();
      if (e.key === "Escape") {
        if (state.helpQ) state.helpQ = "";
        else state.overlay = null;
      } else if (e.key === "Backspace") state.helpQ = state.helpQ.slice(0, -1);
      else if (["ArrowUp", "ArrowDown", "PageUp", "PageDown"].includes(e.key)) {
        const body = frame.querySelector(".tsk-help-body");
        const dir = e.key === "ArrowUp" || e.key === "PageUp" ? -1 : 1;
        body?.scrollBy({
          top: dir * (e.key.startsWith("Page") ? body.clientHeight : 24),
        });
        return true;
      } else if (e.key.length === 1 && plain(e)) state.helpQ += e.key;
      render();
      return true;
    }
    if (o === "palette") {
      e.preventDefault();
      if (e.key === "Escape") state.overlay = null;
      else if (e.key === "Enter") {
        const cmd = paletteCommands()[state.paletteI];
        state.overlay = null;
        cmd?.run();
      } else if (e.key === "ArrowDown" || e.key === "Tab") state.paletteI++;
      else if (e.key === "ArrowUp")
        state.paletteI = Math.max(0, state.paletteI - 1);
      else if (e.key === "Backspace") {
        state.paletteQ = state.paletteQ.slice(0, -1);
        state.paletteI = 0;
      } else if (e.key.length === 1 && plain(e)) {
        state.paletteQ += e.key;
        state.paletteI = 0;
      }
      render();
      return true;
    }
    if (o === "picker") {
      e.preventDefault();
      const opts = scopeOptions();
      if (e.key === "Escape" || e.key === "q") state.overlay = null;
      else if (e.key === "Enter") {
        state.overlay = null;
        setScope(opts[state.pickerI]);
      } else if (e.key === "ArrowDown" || e.key === "j")
        state.pickerI = Math.min(opts.length - 1, state.pickerI + 1);
      else if (e.key === "ArrowUp" || e.key === "k")
        state.pickerI = Math.max(0, state.pickerI - 1);
      render();
      return true;
    }
    if (o === "parent") {
      e.preventDefault();
      const opts = parentOptions();
      if (e.key === "Escape") state.overlay = null;
      else if (e.key === "Enter") chooseParent(state.parentI);
      else if (e.key === "ArrowDown" || e.key === "Tab")
        state.parentI = Math.min(opts.length - 1, state.parentI + 1);
      else if (e.key === "ArrowUp")
        state.parentI = Math.max(0, state.parentI - 1);
      else if (e.key === "Backspace") {
        state.parentQ = state.parentQ.slice(0, -1);
        state.parentI = 0;
      } else if (e.key.length === 1 && plain(e)) {
        state.parentQ += e.key;
        state.parentI = 0;
      }
      render();
      return true;
    }
    return false;
  }

  function handleQuestionsKey(e) {
    const wide = isWide();
    const page = questionById(state.q.page);
    if (!plain(e)) return false;
    if (e.key === "z") {
      e.preventDefault();
      toggleClosed();
      return true;
    }
    if (e.key === "1" || e.key === "2") {
      e.preventDefault();
      selectTab(e.key === "1" ? "board" : "questions");
      return true;
    }
    if (e.key === "?") {
      e.preventDefault();
      state.overlay = "help";
      state.helpQ = "";
      return true;
    }
    if (!page) {
      const order = questionOrder();
      const i = order.indexOf(state.q.cursor);
      if (e.key === "ArrowDown" || e.key === "j") {
        if (order.length)
          state.q.cursor = order[Math.min(order.length - 1, i + 1)];
      } else if (e.key === "ArrowUp" || e.key === "k") {
        if (order.length) state.q.cursor = order[Math.max(0, i - 1)];
      } else if (
        e.key === "Enter" ||
        (wide && (e.key === "ArrowRight" || e.key === "l"))
      ) {
        if (state.q.cursor) openQuestion(state.q.cursor);
      } else if (e.key === "Escape") {
        // The terminal quits at the list; the browser releases keyboard focus.
        frame.blur();
      } else return false;
      e.preventDefault();
      return true;
    }
    state.message = "";
    switch (e.key) {
      case "Escape":
      case "ArrowLeft":
      case "h":
        if (e.key !== "Escape" && !wide) return false;
        leaveQuestionPage();
        break;
      case "ArrowDown":
      case "j":
      case "ArrowUp":
      case "k": {
        const forward = e.key === "ArrowDown" || e.key === "j";
        if (choosing(page) && !state.q.answer) {
          const n = choices(page).length;
          state.q.options[page.id] = Math.min(
            n - 1,
            Math.max(0, choice(page) + (forward ? 1 : -1)),
          );
        } else
          frame
            .querySelector(".tsk-q-detail, .tsk-list")
            ?.scrollBy({ top: forward ? 24 : -24 });
        break;
      }
      case "J":
        stepQuestion(1);
        break;
      case "K":
        stepQuestion(-1);
        break;
      case "Enter":
        questionEnter();
        break;
      case " ":
        if (page.kind === "approval" && isOpen(page) && page.audience) {
          if (checked(page)) {
            state.q.check = null;
            state.message = "check cleared";
          } else {
            state.q.check = { id: page.id, revision: page.answers.length };
            state.message = "checked · y approves this exact revision";
          }
        }
        break;
      case "y":
        questionApprove(true);
        break;
      case "n":
        questionApprove(false);
        break;
      case "c":
        questionCorrect();
        break;
      case "r":
        questionResend();
        break;
      case "t": {
        const task = taskById(page.taskId);
        if (!task) {
          state.message = `${page.taskId} not found`;
          break;
        }
        state.q.returnQuestion = page.id;
        state.q.returnDepth = listTaskIds().includes(task.id) ? 0 : 1;
        state.q.page = null;
        state.tab = "board";
        if (listTaskIds().includes(task.id)) {
          state.selectedId = task.id;
          state.trail = [];
        } else state.trail = [task.id];
        state.page = true;
        steps.reset();
        break;
      }
      default:
        return false;
    }
    e.preventDefault();
    return true;
  }

  function handlePageKey(e) {
    const task = shownTask();
    if (!task) return false;
    const bare = plain(e);
    if (state.edit) {
      // Edit mode without a text field focused: a step stop or the scope stop.
      if (e.key === "Tab") moveEdit(task, e.shiftKey);
      else if (e.key === "Escape") {
        if (state.quickExpanded) collapseQuickAdd();
        else {
          state.edit = null;
          steps.cancel();
          steps.reset();
        }
      } else if (e.key === "Enter" && (e.shiftKey || e.ctrlKey || e.metaKey)) {
        if (state.quickExpanded) saveExpanded();
        else saveEdit(task);
      } else if (
        state.edit.field === "scope" &&
        [" ", "ArrowLeft", "ArrowRight"].includes(e.key)
      )
        cycleScope(e.key === "ArrowLeft");
      else if (e.key === "Enter" && state.edit.field.startsWith("step:")) {
        const id = state.edit.field.slice(5);
        if (id === "add") steps.begin(task);
        else steps.begin(task, id);
      } else return true;
      e.preventDefault();
      return true;
    }
    if (steps.dirty && e.key === "Escape") {
      steps.cancel();
    } else if (steps.dirty && e.key === "Enter" && e.shiftKey) {
      steps.save(task, true);
      task.updatedAt = clock();
    } else if (
      e.altKey &&
      !e.ctrlKey &&
      !e.metaKey &&
      ["ArrowUp", "ArrowDown"].includes(e.key)
    ) {
      if (steps.shift(task, e.key === "ArrowUp" ? -1 : 1))
        task.updatedAt = clock();
    } else if (
      bare &&
      ["Tab", "ArrowDown", "ArrowUp", "j", "k"].includes(e.key)
    ) {
      moveRing(task, e.shiftKey || ["ArrowUp", "k"].includes(e.key) ? -1 : 1);
    } else if (bare && e.key === "Enter" && steps.selected) {
      activateRing(task);
    } else if (bare && e.key === "Enter" && isWide()) {
      state.page = false;
    } else if (bare && e.key === "a") {
      steps.begin(task);
    } else if (bare && e.key === "e") {
      if (steps.rows(task).some((s) => s.id === steps.selected))
        steps.begin(task, steps.selected);
      else beginEdit(task, "title");
    } else if (
      bare &&
      e.key === "x" &&
      steps.rows(task).some((s) => s.id === steps.selected)
    ) {
      if (steps.remove(task)) task.updatedAt = clock();
    } else if (bare && (e.key === "J" || e.key === "K")) {
      stepTask(e.key === "J" ? 1 : -1);
    } else if (bare && e.key === "P") {
      openParentPicker();
    } else if (bare && e.key === "U") {
      if (hasUnsavedWork()) state.message = "save or cancel edits first";
      else {
        task.parent = null;
        state.message = "parent cleared";
      }
    } else if (
      bare &&
      (e.key === "Escape" ||
        ((e.key === "ArrowLeft" || e.key === "h") && isWide()))
    ) {
      if (e.key === "Escape") closePage();
      else if (!hasUnsavedWork()) state.page = false;
    } else if (bare && STATUS_KEYS.includes(e.key)) {
      clearMarks();
      if (e.key === "s") setStatus("doing");
      else if (e.key === "o") setStatus("todo");
      else if (e.key === "d") setStatus("done");
      else if (e.key === "x") deleteSelected();
      else if (e.key === "u") undo();
      else if (e.key === "f") archiveSelected();
    } else if (bare && e.key === "2") {
      selectTab("questions");
    } else if (bare && e.key === "1") {
      if (!hasUnsavedWork()) {
        state.page = false;
        state.trail = [];
      }
    } else if (bare && e.key === "?") {
      state.overlay = "help";
      state.helpQ = "";
    } else if (bare && e.key === ":") {
      state.overlay = "palette";
      state.paletteQ = "";
      state.paletteI = 0;
    } else {
      if (bare) steps.marked = null;
      return true;
    }
    e.preventDefault();
    return true;
  }

  function handleBoardKey(e) {
    const bare = plain(e);
    const wide = isWide();
    if (e.key === "Escape") {
      if (state.searchPinned) {
        updateSearch("");
        state.searchPinned = false;
      } else frame.blur();
    } else if (bare && e.key === "/") {
      state.searchPinned = false;
      state.overlay = "search";
    } else if (bare && e.key === "?") {
      state.overlay = "help";
      state.helpQ = "";
    } else if (bare && e.key === ":") {
      state.overlay = "palette";
      state.paletteQ = "";
      state.paletteI = 0;
    } else if (bare && e.key === "+") openQuickAdd();
    else if (bare && (e.key === "1" || e.key === "2"))
      selectTab(e.key === "1" ? "board" : "questions");
    else if (bare && e.key === "p") {
      state.overlay = "picker";
      state.pickerI = Math.max(0, scopeOptions().indexOf(state.scope));
    } else if (bare && e.key === "S") {
      clearMarks();
      state.sort = state.sort === "number" ? "project" : "number";
    } else if (bare && (e.key === "z" || e.key === "D")) {
      clearMarks();
      state.drawer = !state.drawer;
    } else if (bare && e.key === "g" && state.drawer) {
      clearMarks();
      state.archivedOpen = !state.archivedOpen;
    } else if (
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey &&
      e.shiftKey &&
      e.key === "M"
    ) {
      if (state.markMode) clearMarks();
      else state.markMode = true;
      state.message = "";
    } else if (
      ["j", "ArrowDown", "k", "ArrowUp"].includes(e.key) &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey
    ) {
      const down = e.key === "j" || e.key === "ArrowDown";
      if (e.shiftKey && !state.markMode) return true;
      if (e.shiftKey) markCurrent();
      move(down ? 1 : -1);
    } else if (bare && e.key === " ") {
      if (state.markMode) toggleMark();
    } else if (bare && e.key === "Enter") {
      openPage();
    } else if (bare && wide && (e.key === "ArrowRight" || e.key === "l")) {
      if (taskById(state.selectedId)) openPage();
    } else if (bare && STATUS_KEYS.includes(e.key)) {
      if (!state.markedIds.size && selectedRow()?.kind !== "task") return true;
      if (e.key === "s") setStatus("doing");
      else if (e.key === "o") setStatus("todo");
      else if (e.key === "d") setStatus("done");
      else if (e.key === "x") deleteSelected();
      else if (e.key === "u") undo();
      else if (e.key === "f") archiveSelected();
    } else if (bare && e.key === "e") {
      const task = taskById(state.selectedId);
      if (task) {
        openPage();
        beginEdit(task, "title");
      }
    } else return false;
    e.preventDefault();
    return true;
  }

  function onKey(e) {
    const el = e.target;
    if (el !== frame) {
      if (el.closest(".pane-bar, .layout-toggle, [data-divider]")) return;
      // Text entry: every key but its own Enter/Escape/Tab is text.
      if (el.matches("input, textarea")) {
        handleTextField(e);
        return;
      }
      // A focused canvas button keeps its native Space/Enter activation.
      if (el.closest("button") && (e.key === " " || e.key === "Enter")) return;
    }
    if (e.key.toLowerCase() !== "x") state.pendingDelete = null;
    if (state.overlay && handleOverlay(e)) return;
    if (
      e.key === "Escape" &&
      state.markMode &&
      plain(e) &&
      !e.shiftKey &&
      state.tab === "board"
    ) {
      e.preventDefault();
      clearMarks();
      state.message = "";
      render();
      return;
    }
    let handled;
    if (state.tab === "questions") handled = handleQuestionsKey(e);
    else if (state.quickExpanded || state.page) handled = handlePageKey(e);
    else {
      if (!["Escape"].includes(e.key)) state.message = "";
      handled = handleBoardKey(e);
    }
    if (handled) render();
  }

  // ── mouse ─────────────────────────────────────────────────────────────────
  let lastClick = { id: null, at: 0 };
  function copyTaskIdentifier(task) {
    const identifier = `T${task.number}`;
    const notice = (message) => {
      state.copyNotice = message;
      setTimeout(() => {
        if (state.copyNotice === message) {
          state.copyNotice = "";
          render();
        }
      }, 2000);
      render();
    };
    if (!navigator.clipboard?.writeText) return notice("copy unavailable");
    navigator.clipboard
      .writeText(identifier)
      .then(() => notice(`copied ${identifier}`))
      .catch(() => notice("copy failed"));
  }
  function runVerb(id) {
    if (id === "open") openPage();
    else if (id === "doing") setStatus("doing");
    else if (id === "todo") setStatus("todo");
    else if (id === "done") setStatus("done");
    else if (id === "undo") undo();
    else if (id === "capture") openQuickAdd();
    else if (id === "help") {
      state.overlay = "help";
      state.helpQ = "";
    } else if (id === "edit" && shownTask()) {
      state.page = true;
      beginEdit(shownTask(), "title");
    } else if (id === "close") closePage();
  }
  function runHint(id) {
    if (id === "save") saveDraft(false);
    else if (id === "details") expandQuickAdd();
    else if (id === "close") state.overlay = null;
    else if (id === "save-expanded") saveExpanded();
    else if (id === "next-field") moveEdit(state.capture, false);
    else if (id === "collapse") collapseQuickAdd();
    else if (id === "pin") {
      state.searchPinned = Boolean(state.searchQuery.trim());
      state.overlay = null;
    } else if (id === "clear") {
      updateSearch("");
      state.searchPinned = false;
      state.overlay = null;
    }
  }

  root.addEventListener("click", (e) => {
    const t = e.target;
    const hit = (selector) => t.closest(selector);
    if (!hit('[data-verb="delete"]')) state.pendingDelete = null;
    let node;
    if ((node = hit("[data-close]"))) {
      state.overlay = null;
      frame.focus();
    } else if ((node = hit("[data-cmd]")) && state.overlay === "palette") {
      const cmd = paletteCommands().find((c) => c.id === node.dataset.cmd);
      state.overlay = null;
      cmd?.run();
    } else if ((node = hit("[data-pick]"))) {
      state.overlay = null;
      setScope(node.dataset.pick);
    } else if ((node = hit("[data-parent-option]"))) {
      chooseParent(Number(node.dataset.parentOption));
    } else if ((node = hit("[data-tab]"))) {
      selectTab(node.dataset.tab);
    } else if (hit("[data-scope]")) {
      state.overlay = "picker";
      state.pickerI = Math.max(0, scopeOptions().indexOf(state.scope));
    } else if (hit("[data-sort]")) {
      state.sort = state.sort === "number" ? "project" : "number";
    } else if (hit("[data-waiting]")) {
      selectTab("questions");
    } else if ((node = hit("[data-q-option], [data-q-check], [data-q-key]"))) {
      // A click on the wide preview opens that record before it acts.
      const shown = node.closest("[data-q-page]")?.dataset.qPage;
      if (shown && state.q.page !== shown) openQuestion(shown);
      const key = node.dataset.qCheck ? " " : node.dataset.qKey;
      const q = questionById(state.q.page);
      if (node.dataset.qOption !== undefined) {
        if (q && choosing(q) && !state.q.answer)
          state.q.options[q.id] = Number(node.dataset.qOption);
      } else if (state.q.answer && key === "Enter") {
        state.q.answer.text = root.querySelector("#tsk-answer")?.value ?? "";
        questionEnter();
      } else if (state.q.answer && key === "Escape") {
        state.q.answer = null;
        if (state.q.correcting) state.q.correcting = null;
      } else if (!state.q.answer) {
        handleQuestionsKey({
          key,
          ctrlKey: false,
          metaKey: false,
          altKey: false,
          preventDefault() {},
        });
      }
    } else if ((node = hit("[data-question]"))) {
      openQuestion(node.dataset.question);
    } else if ((node = hit("[data-copy-task]")) && !state.markMode) {
      const task = taskById(node.dataset.copyTask);
      if (task) copyTaskIdentifier(task);
      return;
    } else if ((node = hit("[data-ring]"))) {
      const task = shownTask();
      if (!task || hasUnsavedWork()) return;
      state.page = true;
      steps.selected = node.dataset.ring;
      activateRing(task);
    } else if (
      (node = hit("[data-step], [data-step-add]")) &&
      t.id !== "tsk-step-edit"
    ) {
      const task = shownTask();
      if (!task) return;
      if (steps.editor && !steps.editor.text.trim() && !steps.editor.id)
        steps.cancel();
      else if (steps.editor && !steps.save(task, false)) {
        render();
        return;
      }
      if (!state.quickExpanded) state.page = true;
      if (node.hasAttribute("data-step-add")) steps.begin(task);
      else {
        steps.selected = node.dataset.step;
        steps.marked = null;
      }
    } else if (hit("[data-archived-header]")) {
      clearMarks();
      state.archivedOpen = !state.archivedOpen;
    } else if ((node = hit("[data-task]"))) {
      const id = node.dataset.task;
      if (hasUnsavedWork() && id !== state.selectedId) return;
      if (state.markMode && plain(e) && !e.shiftKey) {
        state.selectedId = id;
        toggleMark();
        lastClick = { id: null, at: 0 };
      } else {
        const now = Date.now();
        const double = lastClick.id === id && now - lastClick.at < 350;
        state.selectedId = id;
        state.trail = [];
        state.q.returnQuestion = null;
        if (double) openPage();
        else if (isWide()) state.page = false;
        lastClick = { id, at: now };
      }
      frame.focus({ preventScroll: true });
    } else if (hit(".tsk-task-column") && isWide() && state.tab === "board") {
      if (shownTask()) state.page = true;
    } else if (hit(".tsk-q-detail") && state.q.cursor && !state.q.page) {
      openQuestion(state.q.cursor);
    } else if (hit("[data-drawer]") && state.tab === "board") {
      clearMarks();
      state.drawer = !state.drawer;
    } else if ((node = hit("[data-hint]"))) {
      const add = document.getElementById("tsk-add");
      if (add) state.draft = add.value;
      runHint(node.dataset.hint);
    } else if ((node = hit("[data-verb]"))) {
      runVerb(node.dataset.verb);
    } else return;
    render();
  });

  frame.addEventListener("keydown", onKey);
  // The landing page's layout toggle resizes the pane; the board returns focus to the list.
  frame.addEventListener("tsk:set-stage", () => {
    if (!hasUnsavedWork() && state.tab === "board") {
      state.page = false;
      state.trail = [];
    }
    if (state.overlay !== "quick") state.overlay = null;
    render();
  });
  new ResizeObserver(() => render()).observe(root);
  frame.addEventListener("focusin", () => frame.classList.add("is-focused"));
  frame.addEventListener("focusout", (e) => {
    if (!frame.contains(e.relatedTarget)) frame.classList.remove("is-focused");
  });

  render();
  frame.dispatchEvent(new CustomEvent("tsk:ready"));
})();
