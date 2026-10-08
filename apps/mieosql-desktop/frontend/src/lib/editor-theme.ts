// SQL editor syntax colors — an Xcode-light palette tuned to the MieoSQL
// design system (blue accent, Apple system grays). Purely presentational:
// applied on top of `basicSetup`, whose default highlighter stays as fallback.

import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";

const mieosqlLight = HighlightStyle.define([
  {
    tag: [tags.keyword, tags.definitionKeyword, tags.operatorKeyword, tags.modifier],
    color: "#9b2393", // Xcode magenta — SELECT · FROM · WHERE
  },
  {
    tag: [tags.string, tags.special(tags.string)],
    color: "#c41a16", // Xcode red — 'paid', '"quoted"'
  },
  {
    tag: [tags.number, tags.bool, tags.null],
    color: "#1c00cf", // Xcode blue — 42 · true · NULL
  },
  {
    tag: [tags.comment, tags.lineComment, tags.blockComment],
    color: "#5d6c79",
  },
  { tag: [tags.operator], color: "#3a3a3c" },
  {
    tag: [tags.typeName, tags.className, tags.namespace],
    color: "#0b4f79",
  },
  {
    tag: [tags.function(tags.variableName), tags.function(tags.propertyName)],
    color: "#6a3d9a",
  },
  {
    tag: [tags.constant(tags.name), tags.labelName, tags.atom],
    color: "#1c00cf",
  },
  {
    tag: [tags.punctuation, tags.separator, tags.bracket, tags.paren, tags.brace],
    color: "#6e6e73",
  },
  { tag: [tags.invalid], color: "#ff3b30" },
]);

/** Drop-in extension: `basicSetup, mieosqlEditorTheme, …`. */
export const mieosqlEditorTheme = syntaxHighlighting(mieosqlLight);

// Xcode-dark palette for `[data-theme="dark"]` (same tags, brighter ink).
// Switched at runtime by EditorPane through a Compartment.
const mieosqlDark = HighlightStyle.define([
  {
    tag: [tags.keyword, tags.definitionKeyword, tags.operatorKeyword, tags.modifier],
    color: "#ff7ab2",
  },
  {
    tag: [tags.string, tags.special(tags.string)],
    color: "#ff8170",
  },
  {
    tag: [tags.number, tags.bool, tags.null],
    color: "#d9c97c",
  },
  {
    tag: [tags.comment, tags.lineComment, tags.blockComment],
    color: "#7f8c98",
  },
  { tag: [tags.operator], color: "#98989d" },
  {
    tag: [tags.typeName, tags.className, tags.namespace],
    color: "#5ccfe6",
  },
  {
    tag: [tags.function(tags.variableName), tags.function(tags.propertyName)],
    color: "#c792ea",
  },
  {
    tag: [tags.constant(tags.name), tags.labelName, tags.atom],
    color: "#d9c97c",
  },
  {
    tag: [tags.punctuation, tags.separator, tags.bracket, tags.paren, tags.brace],
    color: "#98989d",
  },
  { tag: [tags.invalid], color: "#ff453a" },
]);

export { mieosqlDark, mieosqlLight };
