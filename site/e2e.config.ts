// Offline demo acceptance. Build site/dist first and leave loopback port 4180 free.
// Set NATIVE_E2E_ROOT to the shared install and PLAYWRIGHT_BROWSERS_PATH to its
// browsers directory. Run that install's e2e CLI with --config site/e2e.config.ts.
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

if (!process.env.NATIVE_E2E_ROOT)
  throw new Error("NATIVE_E2E_ROOT is required");
const require = createRequire(`${process.env.NATIVE_E2E_ROOT}/package.json`);
const { web } = await import(require.resolve("@e2e-dev/web"));
const site = fileURLToPath(new URL(".", import.meta.url));

export default {
  tests: ["e2e/reports.e2e.ts"],
  targets: [
    {
      name: "office",
      engine: web({
        browser: "chromium",
        viewport: { width: 1400, height: 1100 },
      }),
      app: {
        url: "http://127.0.0.1:4180",
        command: {
          executable: "node",
          args: [fileURLToPath(new URL("parity/preview.mjs", import.meta.url))],
          cwd: site,
          log: ".e2e/logs/reports-demo.log",
        },
      },
    },
  ],
  retries: 0,
  workers: 1,
  trace: "on",
  output: ".e2e/reports",
};
