export function parseCapture(raw, fallbackProject) {
  const parts = raw.trim().split(/\s+/).filter(Boolean);
  let project = fallbackProject;
  const title = [];
  for (let i = 0; i < parts.length; i += 1) {
    const part = parts[i];
    if (part === "!p") {
      const argument = parts[i + 1];
      if (!argument || argument.startsWith("!")) {
        project = null;
      } else {
        i += 1;
        project = argument;
      }
      continue;
    }
    title.push(part);
  }
  return { title: title.join(" "), project };
}
