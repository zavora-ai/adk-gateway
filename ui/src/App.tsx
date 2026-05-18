import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
import Layout from './components/Layout';
import Dashboard from './pages/Dashboard';
import Login from './pages/Login';
import AgentModel from './pages/AgentModel';
import Agents from './pages/Agents';
import Channels from './pages/Channels';
import Sessions from './pages/Sessions';
import AWP from './pages/AWP';
import Integrations from './pages/Integrations';
import ScheduledTasks from './pages/ScheduledTasks';
import Config from './pages/Config';
import Logs from './pages/Logs';
import Memory from './pages/Memory';
import Settings from './pages/Settings';
import Setup from './pages/Setup';
import SetupRedirect from './components/SetupRedirect';
import CodingAgentsPage from './pages/coding-agents/CodingAgentsPage';
import AgentListView from './pages/coding-agents/AgentListView';
import OnboardingWizard from './pages/coding-agents/OnboardingWizard';
import AgentDetailLayout from './pages/coding-agents/AgentDetailLayout';
import TaskHistoryTable from './pages/coding-agents/TaskHistoryTable';
import TaskDetailView from './pages/coding-agents/TaskDetailView';
import CostStatisticsPanel from './pages/coding-agents/CostStatisticsPanel';
import TaskDelegationForm from './pages/coding-agents/TaskDelegationForm';
import AgentConfigurationPanel from './pages/coding-agents/AgentConfigurationPanel';

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/ui/login" element={<Login />} />
        <Route path="/ui/setup" element={<Setup />} />
        <Route path="/ui" element={<Layout />}>
          <Route index element={<SetupRedirect><Dashboard /></SetupRedirect>} />
          <Route path="agent" element={<AgentModel />} />
          <Route path="agents" element={<Agents />} />
          <Route path="channels" element={<Channels />} />
          <Route path="sessions" element={<Sessions />} />
          <Route path="awp" element={<AWP />} />
          <Route path="integrations" element={<Integrations />} />
          <Route path="scheduled-tasks" element={<ScheduledTasks />} />
          <Route path="config" element={<Config />} />
          <Route path="logs" element={<Logs />} />
          <Route path="memory" element={<Memory />} />
          <Route path="settings" element={<Settings />} />
          <Route path="coding-agents" element={<CodingAgentsPage />}>
            <Route index element={<AgentListView />} />
            <Route path="new" element={<OnboardingWizard />} />
            <Route path=":agentId" element={<AgentDetailLayout />}>
              <Route index element={<TaskHistoryTable />} />
              <Route path="tasks/:taskId" element={<TaskDetailView />} />
              <Route path="costs" element={<CostStatisticsPanel />} />
              <Route path="new-task" element={<TaskDelegationForm />} />
              <Route path="settings" element={<AgentConfigurationPanel />} />
            </Route>
          </Route>
        </Route>
        <Route path="*" element={<Navigate to="/ui" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
