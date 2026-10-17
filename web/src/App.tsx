import { Route, Routes } from "react-router-dom";
import { Layout } from "./components/Layout";
import { OverviewPage } from "./pages/Overview";
import { TasksPage } from "./pages/Tasks";
import { TaskDetailPage } from "./pages/TaskDetail";
import { GraphPage } from "./pages/Graph";
import { RepositoriesPage, RepositoryDetailPage } from "./pages/Repositories";
import { ExecutionPage } from "./pages/Execution";
import { AnalyticsPage } from "./pages/Analytics";
import { AutomationPage } from "./pages/Automation";
import { SettingsPage } from "./pages/Settings";
import { Empty } from "./components/ui";

export function App() {
  return (
    <Layout>
      <Routes>
        <Route path="/" element={<OverviewPage />} />
        <Route path="/tasks" element={<TasksPage />} />
        <Route path="/tasks/:ref" element={<TaskDetailPage />} />
        <Route path="/graph" element={<GraphPage />} />
        <Route path="/repositories" element={<RepositoriesPage />} />
        <Route path="/repositories/:ref" element={<RepositoryDetailPage />} />
        <Route path="/execution" element={<ExecutionPage />} />
        <Route path="/analytics" element={<AnalyticsPage />} />
        <Route path="/automation" element={<AutomationPage />} />
        <Route path="/settings" element={<SettingsPage />} />
        <Route path="*" element={<Empty title="Page not found" />} />
      </Routes>
    </Layout>
  );
}
