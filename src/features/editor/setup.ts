// Ekstensi dasar untuk semua editor berkas.
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, foldGutter, foldKeymap, indentOnInput, indentUnit } from "@codemirror/language";
import { lintKeymap } from "@codemirror/lint";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import {
  EditorView,
  crosshairCursor,
  drawSelection,
  dropCursor,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import type { Settings } from "@/ipc";
import { oncodeTheme } from "./theme";

/** Kompartemen yang bisa dikonfigurasi ulang tanpa membuat ulang editor. */
export interface EditorCompartments {
  language: Compartment;
  settings: Compartment;
  readOnly: Compartment;
}

export function createCompartments(): EditorCompartments {
  return { language: new Compartment(), settings: new Compartment(), readOnly: new Compartment() };
}

/** Ekstensi yang bergantung pada pengaturan pengguna. */
export function settingsExtensions(settings: Settings["editor"] | undefined): Extension {
  const tabSize = Math.min(Math.max(settings?.tabSize ?? 4, 1), 16);
  const unit = settings?.insertSpaces === false ? "\t" : " ".repeat(tabSize);
  return [
    EditorState.tabSize.of(tabSize),
    indentUnit.of(unit),
    settings?.wordWrap ? EditorView.lineWrapping : [],
    settings?.lineNumbers === false ? [] : [lineNumbers(), highlightActiveLineGutter()],
  ];
}

export function baseExtensions(compartments: EditorCompartments, settings: Settings["editor"] | undefined, options: { large: boolean; readOnly: boolean }): Extension {
  return [
    compartments.settings.of(settingsExtensions(settings)),
    highlightSpecialChars(),
    history(),
    drawSelection(),
    dropCursor(),
    EditorState.allowMultipleSelections.of(true),
    rectangularSelection(),
    crosshairCursor(),
    highlightActiveLine(),
    options.large ? [] : [foldGutter({ openText: "▾", closedText: "▸" }), indentOnInput(), bracketMatching(), closeBrackets(), autocompletion(), highlightSelectionMatches()],
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap, ...completionKeymap, ...lintKeymap, indentWithTab]),
    compartments.readOnly.of(EditorState.readOnly.of(options.readOnly)),
    compartments.language.of([]),
    oncodeTheme,
  ];
}
