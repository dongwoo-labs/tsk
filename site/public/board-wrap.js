// Same word-boundary rule as ui/edit.rs: retain whitespace at a soft break;
// hard-break only a token that cannot fit. Width is measured in painted cells.
function displayCells(character) {
  return /\p{Mark}/u.test(character)
    ? 0
    : /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}\p{Extended_Pictographic}]/u.test(
          character,
        )
      ? 2
      : 1;
}

export function wrapText(value, width) {
  width = Math.max(1, width);
  return String(value)
    .split(/\r\n|\r|\n/)
    .flatMap((line) => {
      const chars = [...line];
      const rows = [];
      let start = 0;
      while (start < chars.length) {
        let end = start,
          used = 0,
          soft;
        while (end < chars.length) {
          const size = displayCells(chars[end]);
          if (used + size > width && end > start) {
            end = soft ?? end;
            break;
          }
          used += size;
          if (/\s/u.test(chars[end])) soft = end + 1;
          end++;
          if (used > width) break;
        }
        rows.push(chars.slice(start, end).join(""));
        start = end;
      }
      return rows.length ? rows : [""];
    });
}

// Read-only markdown, as ui/markdown.rs renders notes and question bodies: markers
// hidden, then wrapped on the rendered text under a hanging indent. Text only; the
// demo paints no per-span styles.
const inlinePlain = (text) =>
  text
    .replace(/\\([^\w\s])/g, (_, c) => `\u0000${c.charCodeAt(0)}\u0000`)
    .replace(/!?\[([^\][]*)\]\(([^\s()]+)\)/g, (_, label, url) =>
      !label || label === url ? url : `${label} (${url})`,
    )
    .replace(/<((?:https?:\/\/|mailto:)[^\s<>]+)>/g, "$1")
    .replace(/\*\*(\S(?:.*?\S)?)\*\*/g, "$1")
    .replace(/~~(\S(?:.*?\S)?)~~/g, "$1")
    .replace(/\*(?![\s*])([^*]*?[^\s*])\*/g, "$1")
    .replace(
      /(^|[^\p{L}\p{N}])_(?![\s_])([^_]*?[^\s_])_(?![\p{L}\p{N}])/gu,
      "$1$2",
    )
    .replace(/\u0000(\d+)\u0000/g, (_, code) => String.fromCharCode(code));

// Code spans stay verbatim, ticks kept.
const inlineMarkdown = (text) =>
  text
    .split(/(`[^`]*`)/)
    .map((part, i) => (i % 2 ? part : inlinePlain(part)))
    .join("");

export function markdownRows(value, width, cells = displayCells) {
  width = Math.max(1, width);
  const lines = String(value).split(/\r\n|\r|\n/);
  const out = [];
  const isRow = (line) => /^\s*\|.*\|/.test(line ?? "");
  const isDivider = (line) => /^\s*\|[\s|:-]*-[\s|:-]*$/.test(line ?? "");
  let fence = false;
  lines.forEach((raw, index) => {
    const line = raw.replace(/\s+$/, "");
    const fenceLine = /^\s*```/.test(line);
    if (fenceLine || fence) {
      fence = fence !== fenceLine;
      out.push(...wrapText(line, width, cells));
      return;
    }
    if (isDivider(line) && isRow(lines[index - 1])) return;
    let first = /^ */.exec(line)[0];
    let rest = line.slice(first.length);
    while (rest.startsWith(">")) {
      first += "│ ";
      rest = rest.slice(1).replace(/^ /, "");
    }
    let cont = first;
    let body = rest;
    let match;
    if (/^([-*_])( *\1){2,} *$/.test(rest)) {
      out.push(first + "─".repeat(Math.max(0, width - first.length)));
      return;
    } else if ((match = /^#{1,6}(?: +(.*))?$/.exec(rest))) {
      body = match[1] ?? "";
    } else if ((match = /^[-*+] \[([ xX])\](?: (.*))?$/.exec(rest))) {
      first += match[1] === " " ? "☐ " : "☑ ";
      cont += "  ";
      body = match[2] ?? "";
    } else if ((match = /^([-*+]|\d{1,9}[.)]) (.*)$/.exec(rest))) {
      first += `${match[1]} `;
      cont += " ".repeat(match[1].length + 1);
      body = match[2];
    } else if (isRow(rest)) {
      body = rest
        .trim()
        .replace(/^\|/, "")
        .replace(/(?<!\\)\|$/, "")
        .split(/(?<!\\)\|/)
        .map((cell) => cell.trim())
        .join(" │ ");
    }
    const room = Math.max(1, width - Math.max(first.length, cont.length));
    wrapText(inlineMarkdown(body), room, cells).forEach((row, i) =>
      out.push((i ? cont : first) + row),
    );
  });
  return out.length ? out : [""];
}
