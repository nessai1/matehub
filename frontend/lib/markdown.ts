// ── Toolbar: insert markdown around selection ────

export interface MarkdownAction {
  prefix: string;
  suffix: string;
  block?: boolean; // line-level (list items, etc.)
  placeholder: string;
}

export const mdActions: Record<string, MarkdownAction> = {
  bold:          { prefix: "**", suffix: "**", placeholder: "bold text" },
  italic:        { prefix: "*", suffix: "*", placeholder: "italic text" },
  underline:     { prefix: "__", suffix: "__", placeholder: "underlined text" },
  link:          { prefix: "[", suffix: "](url)", placeholder: "link text" },
  code:          { prefix: "`", suffix: "`", placeholder: "code" },
  orderedList:   { prefix: "1. ", suffix: "", block: true, placeholder: "list item" },
  unorderedList: { prefix: "- ", suffix: "", block: true, placeholder: "list item" },
};

/**
 * Apply a markdown action to a textarea.
 * Wraps selected text or inserts placeholder at cursor.
 */
export function applyMarkdown(
  textarea: HTMLTextAreaElement,
  action: MarkdownAction,
): string {
  const { selectionStart, selectionEnd, value } = textarea;
  const selected = value.slice(selectionStart, selectionEnd);
  const text = selected || action.placeholder;

  let insert: string;
  if (action.block) {
    // For lists: ensure we're at the start of a line
    const beforeCursor = value.slice(0, selectionStart);
    const needsNewline = beforeCursor.length > 0 && !beforeCursor.endsWith("\n");
    insert = (needsNewline ? "\n" : "") + action.prefix + text + action.suffix;
  } else {
    insert = action.prefix + text + action.suffix;
  }

  const newValue = value.slice(0, selectionStart) + insert + value.slice(selectionEnd);

  // Schedule cursor placement after React re-renders
  requestAnimationFrame(() => {
    textarea.focus();
    if (selected) {
      textarea.setSelectionRange(selectionStart + insert.length, selectionStart + insert.length);
    } else {
      // Select the placeholder so user can type over it
      const placeholderStart = selectionStart + (action.block && value.length > 0 && !value.slice(0, selectionStart).endsWith("\n") ? 1 : 0) + action.prefix.length;
      textarea.setSelectionRange(placeholderStart, placeholderStart + text.length);
    }
  });

  return newValue;
}

// ── Render: parse markdown to React-safe HTML ────

import DOMPurify from "dompurify";

/**
 * Parse a subset of markdown to HTML for chat messages.
 * Supports: **bold**, *italic*, __underline__, `code`,
 * ```code blocks```, [links](url), - lists, 1. lists
 *
 * Output is sanitized with DOMPurify before return. Two specific holes
 * the sanitizer is closing — both reachable from a hand-crafted chat
 * message before this PR:
 *
 *   1. `[x](javascript:alert(...))` — markdown link with a script-y
 *      scheme. Without sanitization the regex below produces
 *      `<a href="javascript:...">x</a>`; clicking it executes in our
 *      origin → token theft. DOMPurify's `ALLOWED_URI_REGEXP` strips
 *      every URL whose scheme isn't in the allowlist below.
 *
 *   2. Unescaped `"` inside URLs — the hand-rolled HTML escape on the
 *      first three lines covers `<`, `>`, `&` but not `"` / `'`, so a
 *      URL like `https://a.com/" onload="alert(1)` breaks out of the
 *      `href="..."` attribute and injects an event handler. DOMPurify
 *      strips disallowed attributes regardless of how they got there.
 *
 * The hand-rolled regex stage stays for readability — DOMPurify is the
 * single trust boundary at the end.
 */
export function renderMarkdown(text: string): string {
  // Escape HTML
  let html = text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");

  // Code blocks (``` ... ```) -- must be before inline patterns
  html = html.replace(/```([\s\S]*?)```/g, '<pre class="my-1 rounded bg-muted/50 px-2 py-1 text-xs font-mono overflow-x-auto"><code>$1</code></pre>');

  // Inline code
  html = html.replace(/`([^`]+)`/g, '<code class="rounded bg-muted/50 px-1 py-0.5 text-xs font-mono">$1</code>');

  // Bold **text**
  html = html.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");

  // Italic *text* (but not inside **)
  html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, "<em>$1</em>");

  // Underline __text__
  html = html.replace(/__(.+?)__/g, '<span class="underline">$1</span>');

  // Links [text](url) -- no target="_blank", intercepted by click handler.
  // The two suppressions below silence IDE-only false positives:
  //   * RegExpRedundantEscape — `\]` inside a character class works
  //     identically with or without escape, and we keep it for clarity
  //     across regex flavors.
  //   * HtmlUnknownTarget — `$2`/`$1` are JS regex backreferences in
  //     the replacement string, not file paths the IDE keeps trying
  //     to resolve.
  // noinspection RegExpRedundantEscape
  // noinspection HtmlUnknownTarget
  html = html.replace(
    /\[([^\]]+)\]\(([^)]+)\)/g,
    '<a href="$2" data-external-link class="text-primary underline underline-offset-2 hover:text-primary/80 cursor-pointer">$1</a>',
  );

  // Auto-link bare URLs (not already inside an <a> tag).
  // noinspection HtmlUnknownTarget
  html = html.replace(
    /(?<!href="|">)(https?:\/\/[^\s<]+)/g,
    '<a href="$1" data-external-link class="text-primary underline underline-offset-2 hover:text-primary/80 cursor-pointer">$1</a>',
  );

  // Line-level: convert lines starting with - or * to list items
  const lines = html.split("\n");
  let inUl = false;
  let inOl = false;
  const processed: string[] = [];

  for (const line of lines) {
    const ulMatch = line.match(/^[-*]\s+(.+)/);
    const olMatch = line.match(/^\d+\.\s+(.+)/);

    if (ulMatch) {
      if (!inUl) { processed.push('<ul class="list-disc pl-4 my-0.5">'); inUl = true; }
      processed.push(`<li>${ulMatch[1]}</li>`);
    } else if (olMatch) {
      if (!inOl) { processed.push('<ol class="list-decimal pl-4 my-0.5">'); inOl = true; }
      processed.push(`<li>${olMatch[1]}</li>`);
    } else {
      if (inUl) { processed.push("</ul>"); inUl = false; }
      if (inOl) { processed.push("</ol>"); inOl = false; }
      processed.push(line);
    }
  }
  if (inUl) processed.push("</ul>");
  if (inOl) processed.push("</ol>");

  html = processed.join("\n");

  // Convert remaining newlines to <br>
  html = html.replace(/\n/g, "<br>");

  // Final trust boundary. The allowlists below mirror exactly the tags
  // and attributes the regex stage above can produce — anything else
  // (script, iframe, on* handlers, javascript:/data: URLs, style=) gets
  // stripped. Adding new markdown features means extending this list,
  // not bypassing it.
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS: [
      "strong",
      "em",
      "span",
      "code",
      "pre",
      "a",
      "ul",
      "ol",
      "li",
      "br",
    ],
    ALLOWED_ATTR: ["class", "href", "data-external-link"],
    // Block javascript:, data:, vbscript:, and any other surprising
    // scheme. Keeping http/https/mailto covers every URL we expect a
    // chat message to legitimately contain.
    ALLOWED_URI_REGEXP: /^(?:https?|mailto):/i,
  });
}
