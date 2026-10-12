// Quick-add title tokens. Only `!p` remains: tasks carry no thread label since store v8,
// so `!t` is ordinary title text, as in the app.
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
