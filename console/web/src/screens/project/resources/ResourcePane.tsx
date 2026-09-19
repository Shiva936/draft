import { useEffect, useRef, useState } from "react";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, basicSetup } from "codemirror";
import { cspNonce } from "../../../lib/nonce";
import { Icon } from "../../../icons";
import { classificationSummary, lineEnding } from "../../../lib/format";
import type { ResourceClassification } from "../../../lib/format";
import { loadGrammar } from "./viewers/text-grammars";

const language = new Compartment();

export function ResourcePane({
  resourceId,
  resource,
  grammar,
  content,
  readOnly,
  onChange,
  onSave,
}: {
  /** Identity, not a path: this only keys the view instance. */
  resourceId: string;
  /** The resource's derived classification, for the status-bar label. */
  resource: ResourceClassification | undefined;
  /**
   * The grammar the resolved presentation binding asked for, or `null`. An
   * extension-free Draft resolves nothing and renders plain text, rather than
   * the Console guessing a language from a name.
   */
  grammar: string | null;
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
    void loadGrammar(grammar).then((extensions) => {
      if (!cancelled && extensions.length > 0) {
        instance.dispatch({ effects: language.reconfigure(extensions) });
      }
    });

    return () => {
      cancelled = true;
      instance.destroy();
      view.current = null;
    };
    // Remounting on resource id keeps each opened resource's view state
    // independent.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resourceId, readOnly, grammar]);

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
      <div className="resource-status">
        <span>{classificationSummary(resource)}</span>
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
