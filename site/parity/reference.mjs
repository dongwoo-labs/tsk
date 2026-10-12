import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

export function referencePath(name) {
  return process.env.TSK_PARITY_OUTPUT
    ? resolve(process.env.TSK_PARITY_OUTPUT, name)
    : new URL(`../parity-reference/${name}`, import.meta.url);
}

export async function readReference(name) {
  const path = referencePath(`${name}.json`);
  try {
    return JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") {
      throw new Error(
        `Missing parity reference ${name}.json. Run npm run parity:reference from site/ first.`,
      );
    }
    throw error;
  }
}
