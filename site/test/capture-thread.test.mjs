import assert from "node:assert/strict";
import test from "node:test";

import { parseCapture } from "../public/capture.js";

test("demo capture preserves thread-looking title text", () => {
  for (const name of ["V0.0.6", "start_name", "Site-Docs"]) {
    const title = `Ship it !t ${name}`;
    assert.deepEqual(parseCapture(title, null), { title, project: null });
  }
});
