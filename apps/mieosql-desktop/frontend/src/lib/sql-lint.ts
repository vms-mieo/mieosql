// Live SQL lint — positioned diagnostics + quick fixes for the editor.
//
// The same conservative heuristics as `autofixSql`, but every issue keeps its
// exact range so the editor can mark the offending line ("!" in the gutter)
// and offer a one-click fix in the tooltip, IDE-style. Only high-confidence
// problems are reported, so valid SQL is never nagged about.

import type { Diagnostic } from "@codemirror/lint";
import type { EditorView } from "@codemirror/view";

import type { Engine } from "./bindings";
import { isCodePosition, scanSqlSegments } from "./sql-format";
import { GLUED, isIdentifierPosition, suggestKeyword } from "./sql-fix";

/** Engines whose editor content is not SQL — nothing to lint. */
const NON_SQL: readonly Engine[] = ["mongodb", "redis", "kafka"];

type QuickFix =
  | { name: string; kind: "replace"; text: string }
  | { name: string; kind: "remove" }
  | { name: string; kind: "append"; text: string }
  | { name: string; kind: "close-parens"; count: number };

function makeDiagnostic(
  from: number,
  to: number,
  severity: "error" | "warning",
  message: string,
  fix?: QuickFix,
): Diagnostic {
  const diagnostic: Diagnostic = { from, to, severity, message, source: "SQL" };
  if (fix) {
    diagnostic.actions = [
      {
        name: fix.name,
        apply: (view: EditorView, from: number, to: number) => {
          switch (fix.kind) {
            case "replace":
              view.dispatch({ changes: { from, to, insert: fix.text } });
              break;
            case "remove":
              view.dispatch({ changes: { from, to, insert: "" } });
              break;
            case "append":
              view.dispatch({
                changes: { from: view.state.doc.length, insert: fix.text },
              });
              break;
            case "close-parens": {
              const text = view.state.doc.toString();
              const semi = /\s*;\s*$/.exec(text);
              const at = semi ? semi.index : text.length;
              view.dispatch({
                changes: { from: at, insert: ")".repeat(fix.count) },
              });
              break;
            }
          }
          view.focus();
        },
      },
    ];
  }
  return diagnostic;
}

const QUOTE_CLOSE: Record<string, string | undefined> = {
  single: "'",
  double: '"',
  backtick: "`",
  bracket: "]",
};

const OPERATOR_RE = /===|==|=<|=>/g;
const OPERATOR_FIX: Record<string, { text: string; message: string } | undefined> = {
  "===": { text: "=", message: `"===" is not SQL — use "="` },
  "==": { text: "=", message: `"==" is not SQL — use "="` },
  "=<": { text: "<=", message: `"=<" is not SQL — did you mean "<="?` },
  "=>": { text: ">=", message: `"=>" is not SQL — did you mean ">="?` },
};

const GLUED_RE = new RegExp(`\\b(${Object.keys(GLUED).join("|")})\\b`, "gi");
const CLAUSE_RE =
  /,(\s+(?:FROM|WHERE|GROUP|ORDER|HAVING|LIMIT|VALUES|SET|UNION|EXCEPT|INTERSECT|JOIN|LEFT|RIGHT|INNER|FULL|CROSS)\b)/gi;

/**
 * Lint `sql` for `engine`. Returns diagnostics sorted by position; overlapping
 * reports from different rules are collapsed to the first (highest-priority)
 * one so a single line never stacks markers.
 */
export function lintSql(engine: Engine | null, sql: string): Diagnostic[] {
  if (!sql.trim()) return [];
  if (engine !== null && NON_SQL.includes(engine)) return [];

  const segments = scanSqlSegments(engine, sql);
  const found: Diagnostic[] = [];

  // 1) Unterminated string / quoted identifier / block comment -----------
  const last = segments[segments.length - 1];
  if (last && last.to === sql.length) {
    const quote = QUOTE_CLOSE[last.kind];
    if (quote) {
      const body = last.text;
      const closed =
        body.length > 1 &&
        body.endsWith(quote) &&
        body[body.length - 2] !== "\\";
      if (!closed) {
        found.push(
          makeDiagnostic(
            last.from,
            sql.length,
            "error",
            `Unterminated string — missing closing ${quote}`,
            { name: `Add closing ${quote}`, kind: "append", text: quote },
          ),
        );
      }
    } else if (last.kind === "block_comment" && !last.text.endsWith("*/")) {
      found.push(
        makeDiagnostic(
          last.from,
          sql.length,
          "error",
          "Unterminated block comment — missing */",
          { name: "Close the comment", kind: "append", text: " */" },
        ),
      );
    }
  }

  // 2) Parentheses --------------------------------------------------------
  const open: number[] = [];
  for (const seg of segments) {
    if (seg.kind !== "code") continue;
    for (let i = 0; i < seg.text.length; i++) {
      const ch = seg.text[i];
      const pos = seg.from + i;
      if (ch === "(") {
        open.push(pos);
      } else if (ch === ")") {
        if (open.length > 0) open.pop();
        else {
          found.push(
            makeDiagnostic(pos, pos + 1, "warning", "Unmatched ')'", {
              name: "Remove ')'",
              kind: "remove",
            }),
          );
        }
      }
    }
  }
  for (const pos of open) {
    found.push(
      makeDiagnostic(pos, pos + 1, "error", "Unclosed '('", {
        name: open.length === 1 ? `Add closing ")"` : `Add ${open.length} closing ")"`,
        kind: "close-parens",
        count: open.length,
      }),
    );
  }

  // 3) Code-only token rules ---------------------------------------------
  for (const seg of segments) {
    if (seg.kind !== "code") continue;
    const code = seg.text;

    for (const m of code.matchAll(GLUED_RE)) {
      const spaced = GLUED[m[0].toUpperCase()];
      if (!spaced) continue;
      const text = m[0] === m[0].toLowerCase() ? spaced.toLowerCase() : spaced;
      const at = seg.from + (m.index ?? 0);
      found.push(
        makeDiagnostic(
          at,
          at + m[0].length,
          "warning",
          `Glued keywords — did you mean "${spaced}"?`,
          { name: `Split into ${spaced}`, kind: "replace", text },
        ),
      );
    }

    for (const m of code.matchAll(OPERATOR_RE)) {
      const fix = OPERATOR_FIX[m[0]];
      if (!fix) continue;
      const at = seg.from + (m.index ?? 0);
      found.push(
        makeDiagnostic(at, at + m[0].length, "warning", fix.message, {
          name: `Use "${fix.text}"`,
          kind: "replace",
          text: fix.text,
        }),
      );
    }

    for (const m of code.matchAll(/(?:,\s*){2,}/g)) {
      const at = seg.from + (m.index ?? 0);
      found.push(
        makeDiagnostic(at, at + m[0].length, "warning", "Duplicate commas", {
          name: "Collapse into one comma",
          kind: "replace",
          text: ",",
        }),
      );
    }

    for (const m of code.matchAll(/,(\s*\))/g)) {
      const at = seg.from + (m.index ?? 0);
      found.push(
        makeDiagnostic(at, at + m[0].length, "warning", "Dangling comma before ')'", {
          name: "Remove the comma",
          kind: "replace",
          text: m[1],
        }),
      );
    }

    for (const m of code.matchAll(CLAUSE_RE)) {
      const at = seg.from + (m.index ?? 0);
      const clause = m[1].trim();
      found.push(
        makeDiagnostic(
          at,
          at + m[0].length,
          "warning",
          `Dangling comma before "${clause}"`,
          { name: "Remove the comma", kind: "replace", text: m[1] },
        ),
      );
    }

    for (const m of code.matchAll(/[A-Za-z_][A-Za-z0-9_$]*/g)) {
      const offset = m.index ?? 0;
      if (isIdentifierPosition(code, offset)) continue;
      const suggestion = suggestKeyword(m[0]);
      if (!suggestion) continue;
      const at = seg.from + offset;
      found.push(
        makeDiagnostic(
          at,
          at + m[0].length,
          "warning",
          `Unknown keyword "${m[0]}" — did you mean ${suggestion}?`,
          { name: `Replace with ${suggestion}`, kind: "replace", text: suggestion },
        ),
      );
    }
  }

  // 4) Trailing comma at the very end of the statement --------------------
  const trailing = /,\s*$/.exec(sql);
  if (trailing && isCodePosition(engine, sql, trailing.index)) {
    found.push(
      makeDiagnostic(
        trailing.index,
        trailing.index + 1,
        "warning",
        "Trailing comma at the end of the statement",
        { name: "Remove the comma", kind: "remove" },
      ),
    );
  }

  // 5) Dangling AND/OR at the very end ------------------------------------
  const dangling = /\b(AND|OR)\s*$/i.exec(sql);
  if (dangling && isCodePosition(engine, sql, dangling.index)) {
    found.push(
      makeDiagnostic(
        dangling.index,
        dangling.index + dangling[1].length,
        "warning",
        `Dangling "${dangling[1]}" at the end of the statement`,
        { name: `Remove "${dangling[1]}"`, kind: "remove" },
      ),
    );
  }

  // Collapse overlapping ranges (rule order above = priority).
  found.sort((a, b) => a.from - b.from || b.to - a.to);
  const out: Diagnostic[] = [];
  let lastTo = -1;
  for (const diagnostic of found) {
    if (diagnostic.from < lastTo) continue;
    out.push(diagnostic);
    lastTo = diagnostic.to;
  }
  return out;
}
