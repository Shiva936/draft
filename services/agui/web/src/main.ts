import { closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { cpp } from "@codemirror/lang-cpp";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { java } from "@codemirror/lang-java";
import { javascript } from "@codemirror/lang-javascript";
import { json as jsonLanguage } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { sql } from "@codemirror/lang-sql";
import { xml } from "@codemirror/lang-xml";
import { bracketMatching, defaultHighlightStyle, foldGutter, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { EditorState, Extension } from "@codemirror/state";
import { drawSelection, EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers, rectangularSelection } from "@codemirror/view";
import tokens from "./tokens.json";

type PackSummary = {
  pack_id: string;
  name: string;
  intent: string;
  submit_state: string;
  import_state?: string;
};

type PackInspect = {
  manifest: {
    pack_id: string;
    name: string;
    intent: string;
    approval_state: string;
    submit_state: string;
    import_state: string;
    target_workspace_hash: string;
  };
  lifecycle: string;
  verified: boolean;
  symbols_touched?: string[];
  public_api_changed?: string[];
};

type EditorFileEntry = {
  path: string;
  kind: string;
  protected: boolean;
  bytes: number;
};

type EditorFileView = {
  path: string;
  content: string;
  protected: boolean;
  workspace_hash: string;
};

type EditorWorkspace = {
  mode: string;
  workspace_hash: string;
  pending_edits: number;
  files: number;
  status: { text: string };
};

type EditorSearchHit = {
  path: string;
  line: number;
  preview: string;
};

type EditorDiff = {
  path: string;
  base: string;
  unified_diff: string;
  workspace_hash: string;
};

type TaskView = {
  task: { id: string; name: string; goal: string; risk: string; mode: string };
  health: string;
  health_status: { text: string };
  review_status: string;
  review_status_display: { text: string };
  recommended_action: string;
  execution_count: number;
  evidence_count: number;
  produced_packs: string[];
};

type Readiness = {
  ok: boolean;
  blockers: string[];
  verification_receipt_id?: string;
  review_receipt_id?: string;
  approval_ref?: string;
};

type InboxItem = {
  status: string;
  kind: string;
  subject_id: string;
  next_action: string;
};

type DoctorReport = {
  global: { checks: DoctorCheck[] };
  project?: { checks: DoctorCheck[] };
};

type DoctorCheck = {
  name: string;
  ok: boolean;
  detail: string;
};

const csrf = document.querySelector<HTMLMetaElement>('meta[name="draft-csrf"]')?.content ?? "";
const bearer = document.querySelector<HTMLMetaElement>('meta[name="draft-bearer"]')?.content ?? "";
const appRoot = document.querySelector<HTMLDivElement>("#app");

if (!appRoot) {
  throw new Error("missing app root");
}

const app = appRoot;

let packs: PackSummary[] = [];
let currentPack: string | null = null;
let currentTab = "detail";
let editorView: EditorView | null = null;
let editorPath: string | null = null;
let editorBase = "";
let editorWorkspaceHash = "";
let editorSelection: { from: number; to: number } | null = null;

async function api<T>(path: string, opts: RequestInit = {}): Promise<T> {
  const headers = new Headers(opts.headers);
  if (bearer) {
    headers.set("authorization", `Bearer ${bearer}`);
  }
  const response = await fetch(path, { ...opts, headers });
  if (!response.ok) {
    throw new Error((await response.text()) || response.statusText);
  }
  const contentType = response.headers.get("content-type") ?? "";
  if (contentType.includes("application/json")) {
    return (await response.json()) as T;
  }
  return (await response.text()) as T;
}

function post<T>(path: string, body: unknown): Promise<T> {
  return api<T>(path, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-draft-csrf": csrf,
    },
    body: JSON.stringify(body),
  });
}

function esc(value: unknown): string {
  return String(value ?? "").replace(/[&<>"]/g, (char) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
  })[char] ?? char);
}

function renderShell(): void {
  app.innerHTML = `
    <header>
      <h1>DRAFT</h1>
      <nav>
        <button data-view="packs">Packs</button>
        <button data-view="editor">Editor</button>
        <button data-view="tasks">Tasks</button>
        <button data-view="inbox">Inbox</button>
        <button data-view="doctor">Doctor</button>
        <button data-view="events">Events</button>
      </nav>
      <span id="status"></span>
    </header>
    <main>
      <aside id="list"></aside>
      <section id="detail"></section>
    </main>`;
  document.querySelectorAll<HTMLButtonElement>("nav button").forEach((button) => {
    button.addEventListener("click", () => route(button.dataset.view ?? "packs"));
  });
}

async function route(view: string): Promise<void> {
  setStatus("");
  if (view === "editor") return showEditor();
  if (view === "tasks") return showTasks();
  if (view === "inbox") return showInbox();
  if (view === "doctor") return showDoctor();
  if (view === "events") return showEvents();
  return loadPacks();
}

function setStatus(text: string): void {
  const status = document.querySelector<HTMLElement>("#status");
  if (status) status.textContent = text;
}

async function loadPacks(): Promise<void> {
  packs = await api<PackSummary[]>("/packs");
  setStatus(`${packs.length} packs`);
  renderPackList();
  const detail = document.querySelector<HTMLElement>("#detail");
  if (detail && !currentPack) detail.innerHTML = `<p class="muted">Select a pack to review.</p>`;
}

function renderPackList(): void {
  const list = document.querySelector<HTMLElement>("#list");
  if (!list) return;
  list.innerHTML = packs.map((pack) => `
    <button class="row ${currentPack === pack.pack_id ? "active" : ""}" data-pack="${esc(pack.pack_id)}">
      <strong>${esc(pack.name)}</strong>
      <span>${esc(pack.pack_id)} · ${esc(pack.intent)} · ${esc(pack.submit_state)}</span>
    </button>`).join("") || `<p class="muted padded">No packs yet.</p>`;
  list.querySelectorAll<HTMLButtonElement>("[data-pack]").forEach((button) => {
    button.addEventListener("click", () => selectPack(button.dataset.pack ?? ""));
  });
}

async function selectPack(id: string): Promise<void> {
  currentPack = id;
  currentTab = "detail";
  renderPackList();
  await renderPackDetail();
}

async function renderPackDetail(): Promise<void> {
  const detail = document.querySelector<HTMLElement>("#detail");
  if (!detail || !currentPack) return;
  const pack = await api<PackInspect>(`/packs/${encodeURIComponent(currentPack)}`);
  const tabs = ["detail", "readiness", "diff", "risk", "receipts"];
  let body = "";
  if (currentTab === "detail") body = packDetail(pack);
  if (currentTab === "readiness") body = readinessView(await api<Readiness>(`/packs/${currentPack}/readiness`));
  if (currentTab === "diff") body = `<pre>${esc(await api<string>(`/packs/${currentPack}/diff`))}</pre>`;
  if (currentTab === "risk") body = `<pre>${esc(JSON.stringify(await api<unknown>(`/packs/${currentPack}/risk`), null, 2))}</pre>`;
  if (currentTab === "receipts") body = `<pre>${esc(JSON.stringify(await api<unknown>(`/packs/${currentPack}/receipts`), null, 2))}</pre>`;
  detail.innerHTML = `
    <h2>${esc(pack.manifest.name)} <span>${esc(pack.lifecycle)}</span></h2>
    <div class="tabs">${tabs.map((tab) => `<button class="${tab === currentTab ? "active" : ""}" data-tab="${tab}">${tab}</button>`).join("")}</div>
    ${body}
    <div class="actions">
      <button id="approve">Approve</button>
      <button id="reject" class="danger">Reject</button>
    </div>`;
  detail.querySelectorAll<HTMLButtonElement>("[data-tab]").forEach((button) => {
    button.addEventListener("click", () => {
      currentTab = button.dataset.tab ?? "detail";
      void renderPackDetail();
    });
  });
  detail.querySelector<HTMLButtonElement>("#approve")?.addEventListener("click", () => decide(true));
  detail.querySelector<HTMLButtonElement>("#reject")?.addEventListener("click", () => decide(false));
}

function packDetail(pack: PackInspect): string {
  const m = pack.manifest;
  return `<table>
    <tr><td>Pack id</td><td>${esc(m.pack_id)}</td></tr>
    <tr><td>Intent</td><td>${esc(m.intent)}</td></tr>
    <tr><td>Verified</td><td>${pack.verified ? "yes" : "no"}</td></tr>
    <tr><td>Approval</td><td>${esc(m.approval_state)}</td></tr>
    <tr><td>Submit</td><td>${esc(m.submit_state)}</td></tr>
    <tr><td>Import</td><td>${esc(m.import_state)}</td></tr>
    <tr><td>Symbols</td><td>${esc((pack.symbols_touched ?? []).join(", ") || "-")}</td></tr>
    <tr><td>Workspace hash</td><td>${esc(m.target_workspace_hash)}</td></tr>
  </table>`;
}

function readinessView(readiness: Readiness): string {
  return `<p>Submit readiness: <strong class="${readiness.ok ? "ok" : "bad"}">${readiness.ok ? "ready" : "blocked"}</strong></p>
    <table>
      <tr><td>Verification</td><td>${esc(readiness.verification_receipt_id ?? "missing")}</td></tr>
      <tr><td>Review</td><td>${esc(readiness.review_receipt_id ?? "missing")}</td></tr>
      <tr><td>Approval</td><td>${esc(readiness.approval_ref ?? "missing")}</td></tr>
    </table>
    ${(readiness.blockers ?? []).map((blocker) => `<p class="bad">${esc(blocker)}</p>`).join("")}`;
}

async function decide(approve: boolean): Promise<void> {
  if (!currentPack) return;
  await post(`/packs/${currentPack}/${approve ? "approve" : "reject"}`, { reason: "via AG-UI" });
  await loadPacks();
  await renderPackDetail();
}

async function showEditor(): Promise<void> {
  const files = await api<EditorFileEntry[]>("/editor/tree");
  const workspace = await api<EditorWorkspace>("/editor/workspace");
  const list = document.querySelector<HTMLElement>("#list");
  const detail = document.querySelector<HTMLElement>("#detail");
  if (!list || !detail) return;
  list.innerHTML = `
    <input id="fileFilter" placeholder="Filter files" aria-label="Filter files" />
    <input id="projectSearch" placeholder="Search project" aria-label="Search project" />
    <div id="searchRows"></div>
    <div id="fileRows">${fileRows(files)}</div>`;
  detail.innerHTML = `
    <div class="editorHeader">
      <div>
        <h2 id="editorTitle">Workspace editor</h2>
        <p id="editorMeta" class="muted">${esc(workspace.status.text)} · ${workspace.mode} · ${workspace.files} files · ${workspace.pending_edits} pending</p>
      </div>
      <div class="actions">
        <button id="createFile">Create</button>
        <button id="renameFile">Rename</button>
        <button id="deleteFile" class="danger">Delete</button>
        <button id="diffFile">Diff</button>
        <button id="restoreFile">Restore</button>
        <button id="taskFromSelection">Task from selection</button>
        <button id="discardEdit">Discard</button>
        <button id="saveEdit">Save to pack</button>
      </div>
    </div>
    <div id="editor" class="editorSurface"></div>
    <p id="editorFooter" class="muted"></p>`;
  bindFileRows(files);
  list.querySelector<HTMLInputElement>("#fileFilter")?.addEventListener("input", (event) => {
    const query = (event.target as HTMLInputElement).value.toLowerCase();
    const filtered = files.filter((file) => file.path.toLowerCase().includes(query));
    const rows = list.querySelector<HTMLElement>("#fileRows");
    if (rows) rows.innerHTML = fileRows(filtered);
    bindFileRows(filtered);
  });
  list.querySelector<HTMLInputElement>("#projectSearch")?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") void searchProject((event.target as HTMLInputElement).value);
  });
  detail.querySelector<HTMLButtonElement>("#createFile")?.addEventListener("click", createEditorFile);
  detail.querySelector<HTMLButtonElement>("#renameFile")?.addEventListener("click", renameEditorFile);
  detail.querySelector<HTMLButtonElement>("#deleteFile")?.addEventListener("click", deleteEditorFile);
  detail.querySelector<HTMLButtonElement>("#diffFile")?.addEventListener("click", diffEditorFile);
  detail.querySelector<HTMLButtonElement>("#restoreFile")?.addEventListener("click", restoreEditorFile);
  detail.querySelector<HTMLButtonElement>("#discardEdit")?.addEventListener("click", () => {
    if (editorPath) void openEditorFile(editorPath);
  });
  detail.querySelector<HTMLButtonElement>("#saveEdit")?.addEventListener("click", saveEditorFile);
  detail.querySelector<HTMLButtonElement>("#taskFromSelection")?.addEventListener("click", taskFromSelection);
  mountEditor("");
}

function fileRows(files: EditorFileEntry[]): string {
  return files.map((file) => `
    <button class="row ${file.protected ? "protected" : ""}" data-file="${esc(file.path)}">
      <strong>${esc(file.path)}</strong>
      <span>${file.bytes} bytes${file.protected ? " · protected" : ""}</span>
    </button>`).join("") || `<p class="muted padded">No files.</p>`;
}

function bindFileRows(_files: EditorFileEntry[]): void {
  document.querySelectorAll<HTMLButtonElement>("[data-file]").forEach((button) => {
    button.addEventListener("click", () => openEditorFile(button.dataset.file ?? ""));
  });
}

async function openEditorFile(path: string): Promise<void> {
  const file = await api<EditorFileView>(`/editor/file?path=${encodeURIComponent(path)}`);
  editorPath = file.path;
  editorBase = file.content;
  editorWorkspaceHash = file.workspace_hash;
  document.querySelector<HTMLElement>("#editorTitle")!.textContent = file.path;
  document.querySelector<HTMLElement>("#editorMeta")!.textContent = `workspace ${file.workspace_hash}`;
  mountEditor(file.content, languageForPath(file.path), file.protected);
  setEditorFooter("Clean");
}

function mountEditor(content: string, language: Extension = [], readOnly = false): void {
  editorView?.destroy();
  const parent = document.querySelector<HTMLElement>("#editor");
  if (!parent) return;
  editorView = new EditorView({
    parent,
    state: EditorState.create({
      doc: content,
      extensions: [
        lineNumbers(),
        foldGutter(),
        history(),
        drawSelection(),
        rectangularSelection(),
        highlightActiveLine(),
        highlightActiveLineGutter(),
        highlightSelectionMatches(),
        bracketMatching(),
        closeBrackets(),
        indentOnInput(),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, ...completionKeymap, ...closeBracketsKeymap, indentWithTab]),
        EditorView.lineWrapping,
        EditorView.editable.of(!readOnly),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) setEditorFooter(update.state.doc.toString() === editorBase ? "Clean" : "Unsaved changes");
          if (update.selectionSet) {
            const range = update.state.selection.main;
            editorSelection = { from: range.from, to: range.to };
          }
        }),
        editorTheme,
        language,
      ],
    }),
  });
}

async function saveEditorFile(): Promise<void> {
  if (!editorPath || !editorView) return;
  const content = editorView.state.doc.toString();
  const result = await post<{ pack_id: string; workspace_hash: string }>("/editor/save", {
    path: editorPath,
    content,
    pack_name: `editor-${editorPath.replace(/[^a-zA-Z0-9_-]+/g, "-")}`,
  });
  editorBase = content;
  editorWorkspaceHash = result.workspace_hash;
  setEditorFooter(`Saved to pack ${result.pack_id}`);
  await loadPacks();
}

async function createEditorFile(): Promise<void> {
  const path = window.prompt("Path for new file");
  if (!path) return;
  await post("/editor/create", { path, content: "" });
  await showEditor();
  await openEditorFile(path);
}

async function renameEditorFile(): Promise<void> {
  if (!editorPath) return;
  const to = window.prompt("Rename file to", editorPath);
  if (!to || to === editorPath) return;
  await post("/editor/rename", { from: editorPath, to });
  await showEditor();
  await openEditorFile(to);
}

async function deleteEditorFile(): Promise<void> {
  if (!editorPath || !window.confirm(`Delete ${editorPath}?`)) return;
  await post("/editor/delete", { path: editorPath });
  editorPath = null;
  editorBase = "";
  await showEditor();
}

async function searchProject(query: string): Promise<void> {
  const rows = document.querySelector<HTMLElement>("#searchRows");
  if (!rows || !query.trim()) return;
  const hits = await api<EditorSearchHit[]>(`/editor/search?q=${encodeURIComponent(query)}&limit=50`);
  rows.innerHTML = hits.map((hit) => `
    <button class="row" data-file="${esc(hit.path)}">
      <strong>${esc(hit.path)}:${hit.line}</strong>
      <span>${esc(hit.preview)}</span>
    </button>`).join("") || `<p class="muted padded">No matches.</p>`;
  bindFileRows([]);
}

async function diffEditorFile(): Promise<void> {
  if (!editorPath) return;
  const pack = currentPack ?? window.prompt("Pack id for base diff");
  if (!pack) return;
  const diff = await api<EditorDiff>(`/editor/diff?path=${encodeURIComponent(editorPath)}&pack=${encodeURIComponent(pack)}`);
  document.querySelector<HTMLElement>("#editorFooter")!.innerHTML = `<strong>${esc(diff.base)}</strong>`;
  mountEditor(diff.unified_diff);
}

async function restoreEditorFile(): Promise<void> {
  if (!editorPath) return;
  const pack = currentPack ?? window.prompt("Pack id to restore base from");
  if (!pack || !window.confirm(`Restore ${editorPath} from ${pack} base?`)) return;
  const result = await post<{ workspace_hash: string }>("/editor/restore", { path: editorPath, pack });
  editorWorkspaceHash = result.workspace_hash;
  await openEditorFile(editorPath);
}

async function taskFromSelection(): Promise<void> {
  if (!editorPath || !editorView || !editorSelection || editorSelection.from === editorSelection.to) {
    setEditorFooter("Select text before creating a task.");
    return;
  }
  const selected = editorView.state.sliceDoc(editorSelection.from, editorSelection.to);
  const start = editorView.state.doc.lineAt(editorSelection.from);
  const end = editorView.state.doc.lineAt(editorSelection.to);
  await post("/editor/task-from-selection", {
    path: editorPath,
    start_line: start.number,
    end_line: end.number,
    selected_text: selected,
    reason: "AG-UI editor selection",
    workspace_hash: editorWorkspaceHash,
  });
  setEditorFooter("Task created from selection.");
}

function setEditorFooter(text: string): void {
  const footer = document.querySelector<HTMLElement>("#editorFooter");
  if (footer) footer.textContent = text;
}

function languageForPath(path: string): Extension {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (["js", "jsx", "mjs", "cjs"].includes(ext)) return javascript({ jsx: true });
  if (["ts", "tsx"].includes(ext)) return javascript({ typescript: true, jsx: ext === "tsx" });
  if (ext === "rs") return rust();
  if (ext === "py") return python();
  if (["json", "jsonl"].includes(ext)) return jsonLanguage();
  if (["md", "markdown"].includes(ext)) return markdown();
  if (["html", "htm"].includes(ext)) return html();
  if (ext === "css") return css();
  if (["xml", "svg"].includes(ext)) return xml();
  if (["c", "cc", "cpp", "cxx", "h", "hpp"].includes(ext)) return cpp();
  if (ext === "java") return java();
  if (["sql"].includes(ext)) return sql();
  return [];
}

async function showTasks(): Promise<void> {
  const tasks = await api<TaskView[]>("/tasks");
  document.querySelector<HTMLElement>("#detail")!.innerHTML = `<h2>Tasks</h2><table><tr><th>Health</th><th>Task</th><th>Review</th><th>Executions</th><th>Next action</th></tr>${tasks.map((task) => `<tr><td>${esc(task.health_status?.text ?? task.health)}</td><td><strong>${esc(task.task.name)}</strong><br><span class="muted">${esc(task.task.goal)}</span></td><td>${esc(task.review_status_display?.text ?? task.review_status)}</td><td>${task.execution_count}</td><td>${esc(task.recommended_action)}</td></tr>`).join("")}</table>`;
}

async function showInbox(): Promise<void> {
  const items = await api<InboxItem[]>("/inbox");
  document.querySelector<HTMLElement>("#detail")!.innerHTML = `<h2>Inbox</h2><table><tr><th>Status</th><th>Kind</th><th>Subject</th><th>Next action</th></tr>${items.map((item) => `<tr><td>${esc(item.status)}</td><td>${esc(item.kind)}</td><td>${esc(item.subject_id)}</td><td>${esc(item.next_action)}</td></tr>`).join("")}</table>`;
}

async function showDoctor(): Promise<void> {
  const report = await api<DoctorReport>("/doctor");
  const checks = [...(report.global.checks ?? []), ...(report.project?.checks ?? [])];
  document.querySelector<HTMLElement>("#detail")!.innerHTML = `<h2>Doctor</h2><table><tr><th>Status</th><th>Check</th><th>Detail</th></tr>${checks.map((check) => `<tr><td class="${check.ok ? "ok" : "bad"}">${check.ok ? "ok" : "fail"}</td><td>${esc(check.name)}</td><td>${esc(check.detail)}</td></tr>`).join("")}</table>`;
}

async function showEvents(): Promise<void> {
  const events = await api<unknown[]>("/events");
  document.querySelector<HTMLElement>("#detail")!.innerHTML = `<h2>Events</h2><pre>${esc(JSON.stringify(events, null, 2))}</pre>`;
}

const editorTheme = EditorView.theme({
  "&": {
    height: "calc(100vh - 180px)",
    backgroundColor: "#171b22",
    color: "#e6edf3",
    border: "1px solid #272d38",
  },
  ".cm-scroller": { fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace" },
  ".cm-gutters": { backgroundColor: "#11151c", color: "#8b949e", border: "none" },
  ".cm-activeLine": { backgroundColor: "#1f2630" },
  ".cm-activeLineGutter": { backgroundColor: "#1f2630" },
});

const style = document.createElement("style");
style.textContent = `
  :root{--bg:#0e1116;--panel:#171b22;--line:#272d38;--fg:#e6edf3;--muted:${tokens.muted_text};--accent:${tokens.accent};--bad:${tokens.danger};--ok:${tokens.success};--focus:${tokens.focus_ring};--radius-sm:${tokens.radius_sm}px;--radius-md:${tokens.radius_md}px;--space-sm:${tokens.spacing_sm}px;--space-md:${tokens.spacing_md}px}
  *{box-sizing:border-box} body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 -apple-system,Segoe UI,Roboto,sans-serif}
  header{height:50px;display:flex;gap:14px;align-items:center;padding:10px 16px;border-bottom:1px solid var(--line)}
  h1{font-size:15px;margin:0} h2{margin:0 0 var(--space-sm)} nav{display:flex;gap:6px} button{background:var(--panel);color:var(--fg);border:1px solid var(--line);border-radius:var(--radius-sm);padding:7px 10px;cursor:pointer} button:hover{border-color:var(--accent)} button:focus-visible,input:focus-visible{outline:2px solid var(--focus);outline-offset:2px}
  main{display:grid;grid-template-columns:320px 1fr;height:calc(100vh - 50px)} aside{border-right:1px solid var(--line);overflow:auto} section{overflow:auto;padding:16px}
  .row{display:block;width:100%;text-align:left;border:0;border-bottom:1px solid var(--line);border-radius:0;padding:10px 14px}.row span{display:block;color:var(--muted);font-size:12px}.row.active{border-left:3px solid var(--accent)}.row.protected strong{color:var(--bad)}
  input{width:calc(100% - 16px);margin:var(--space-sm);padding:var(--space-sm);background:var(--panel);color:var(--fg);border:1px solid var(--line);border-radius:var(--radius-sm)}
  table{width:100%;border-collapse:collapse}td,th{border-bottom:1px solid var(--line);padding:7px;text-align:left}.tabs,.actions{display:flex;gap:var(--space-sm);flex-wrap:wrap;margin:var(--space-md) 0}.tabs .active{border-color:var(--accent);color:var(--accent)}
  pre{background:var(--panel);border:1px solid var(--line);border-radius:var(--radius-sm);padding:var(--space-md);overflow:auto}.muted,#status{color:var(--muted)}.padded{padding:14px}.bad{color:var(--bad)}.ok{color:var(--ok)}.danger{border-color:var(--bad)}
  .editorHeader{display:flex;justify-content:space-between;gap:12px;align-items:flex-start;margin-bottom:10px}.editorSurface{min-height:55vh}
`;
document.head.appendChild(style);

renderShell();
void loadPacks();
