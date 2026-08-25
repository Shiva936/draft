import { useEffect, useRef, useState } from "react";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, basicSetup } from "codemirror";
import { cspNonce } from "../../../lib/nonce";
import { Icon } from "../../../icons";
import { languageOf, lineEnding } from "../../../lib/format";
import { loadLanguage } from "./languages";

const language = new Compartment();

export function CodePane({
  path,
  content,
  readOnly,
  onChange,
  onSave,
}: {
  path: string;
  content: string;
  readOnly: boolean;
  onChange: (value: string) => void;
  onSave: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const [cursor, setCursor] = useState({ line: 1, column: 1 });

  useEffect(() => {
    if (!host.current) return;
    const instance = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: content,
        extensions: [
          basicSetup,
          ...cspNonce(),
          language.of([]),
          EditorView.editable.of(!readOnly),
          EditorState.readOnly.of(readOnly),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) onChange(update.state.doc.toString());
            if (update.selectionSet || update.docChanged) {
              const head = update.state.selection.main.head;
              const line = update.state.doc.lineAt(head);
              setCursor({ line: line.number, column: head - line.from + 1 });
            }
          }),
        ],
      }),
    });
    view.current = instance;

    let cancelled = false;
    void loadLanguage(path).then((extensions) => {
      if (!cancelled && extensions.length > 0) {
        instance.dispatch({ effects: language.reconfigure(extensions) });
      }
    });

    return () => {
      cancelled = true;
      instance.destroy();
      view.current = null;
    };
    // Remounting on path keeps each opened file's editor state independent.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, readOnly]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        onSave();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onSave]);

  return (
    <>
      <div className="code-host" ref={host} />
      <div className="editor-status">
        <span>{languageOf(path)}</span>
        <span>
          Ln {cursor.line}, Col {cursor.column}
        </span>
        <span>Spaces: 2</span>
        <span>UTF-8</span>
        <span>{lineEnding(content)}</span>
        <span className="spacer" />
        {readOnly && (
          <span>
            <Icon name="shield" size={12} />
            Read only
          </span>
        )}
      </div>
    </>
  );
}
