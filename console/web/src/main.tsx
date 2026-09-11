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
import { Changes } from "./screens/project/work/Changes";
import { ChangeScope } from "./screens/project/work/ChangeScope";
import { ResourceBrowser } from "./screens/project/resources/ResourceBrowser";
import { Events } from "./screens/project/events/Events";
import { Observation } from "./screens/project/observation/Observation";
import { Tools } from "./screens/project/tools/Tools";
import { Baselines } from "./screens/project/baselines/Baselines";
import { BaselineScope } from "./screens/project/baselines/BaselineScope";
import { Publications } from "./screens/project/baselines/Publications";
import { Providers } from "./screens/project/providers/Providers";
import { ProjectExtensions } from "./screens/project/extensions/ProjectExtensions";
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
          {/* §8.3's project information architecture. Work owns Tasks and
              Changes; Observation is a view of Resources; Tools is a view of
              Extensions. The section list itself comes from the generated
              authoritative IA — see ProjectLayout. */}
          <Route path="projects/:workspaceId" element={<ProjectLayout />}>
            <Route index element={<ProjectOverview />} />
            <Route path="work" element={<Tasks />} />
            <Route path="work/changes" element={<Changes />} />
            {/* §8.3 gives a Change and a Baseline their own scopes. Each is
                rendered from the authoritative model for that subject. */}
            <Route path="work/changes/:changeId" element={<ChangeScope />} />
            <Route path="resources" element={<ResourceBrowser />} />
            <Route path="resources/observation" element={<Observation />} />
            <Route path="baselines" element={<Baselines />} />
            <Route path="baselines/publications" element={<Publications />} />
            <Route path="baselines/:baselineId" element={<BaselineScope />} />
            <Route path="activity" element={<Events />} />
            <Route path="providers" element={<Providers />} />
            <Route path="extensions" element={<ProjectExtensions />} />
            <Route path="extensions/tools" element={<Tools />} />
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
