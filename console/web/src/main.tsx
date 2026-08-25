import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { establishSession } from "./api";
import type { Session } from "./contracts";
import { FullLoading } from "./components/states";
import { Shell } from "./shell/Shell";
import { SystemOverview } from "./screens/overview/SystemOverview";
import { Projects } from "./screens/projects/Projects";
import { Inbox } from "./screens/inbox/Inbox";
import { Doctor } from "./screens/doctor/Doctor";
import { Extensions } from "./screens/extensions/Extensions";
import { Settings } from "./screens/settings/Settings";
import { ProjectLayout } from "./screens/project/ProjectLayout";
import { ProjectOverview } from "./screens/project/overview/ProjectOverview";
import { Tasks } from "./screens/project/tasks/Tasks";
import { Editor } from "./screens/project/editor/Editor";
import { Events } from "./screens/project/events/Events";
import { Packs } from "./screens/project/packs/Packs";
import { Pack } from "./screens/project/packs/pack/Pack";
import "./styles.css";

const client = new QueryClient({
  defaultOptions: { queries: { staleTime: 5_000, retry: 1, refetchOnWindowFocus: false } },
});

function Boot() {
  const [session, setSession] = useState<Session | null>(null);
  const [error, setError] = useState<Error | null>(null);
  useEffect(() => {
    establishSession().then(setSession).catch(setError);
  }, []);
  if (error) return <AuthFailure error={error} />;
  if (!session) return <FullLoading label="Securing Console session…" />;
  return <App session={session} />;
}

function App({ session }: { session: Session }) {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Shell session={session} />}>
          <Route index element={<SystemOverview />} />
          <Route path="projects" element={<Projects />} />
          <Route path="inbox" element={<Inbox />} />
          <Route path="doctor" element={<Doctor />} />
          <Route path="extensions" element={<Extensions />} />
          <Route path="settings" element={<Settings />} />
          <Route path="projects/:workspaceId" element={<ProjectLayout />}>
            <Route index element={<ProjectOverview />} />
            <Route path="tasks" element={<Tasks />} />
            <Route path="editor" element={<Editor />} />
            <Route path="events" element={<Events />} />
            <Route path="packs" element={<Packs />}>
              <Route path=":packId/:tab?" element={<Pack />} />
            </Route>
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}

function AuthFailure({ error }: { error: Error }) {
  return (
    <div className="auth-page">
      <span className="brand">
        <img src="/assets/draft-console.png" alt="" width={54} height={54} decoding="async" />
        <span className="brand-word">
          <strong>DRAFT</strong>
          <span>Console</span>
        </span>
      </span>
      <h1>Console session unavailable</h1>
      <p>{error.message}</p>
      <p className="muted">
        Run <code>draft console</code> again to issue a new single-use bootstrap URL.
      </p>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={client}>
      <Boot />
    </QueryClientProvider>
  </StrictMode>,
);
