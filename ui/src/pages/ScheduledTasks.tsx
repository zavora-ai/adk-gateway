import { useState } from 'react';
import { useApi } from '../hooks/useApi';
import { api } from '../api/client';
import StatusBadge from '../components/StatusBadge';
import AlertBanner from '../components/AlertBanner';
import ConfirmDialog from '../components/ConfirmDialog';
import type { CronJobInfo } from '../types';

const SCHEDULE_PRESETS = [
  { label: 'Every 30 seconds', value: '@every 30s' },
  { label: 'Every minute', value: '@every 1m' },
  { label: 'Every 5 minutes', value: '@every 5m' },
  { label: 'Every 15 minutes', value: '@every 15m' },
  { label: 'Every 30 minutes', value: '@every 30m' },
  { label: 'Every hour', value: '@every 1h' },
  { label: 'Every 6 hours', value: '@every 6h' },
  { label: 'Every 12 hours', value: '@every 12h' },
  { label: 'Every 24 hours', value: '@every 24h' },
];

const CHANNEL_OPTIONS = [
  { value: '', label: 'None (webhook)' },
  { value: 'telegram', label: 'Telegram' },
  { value: 'slack', label: 'Slack' },
  { value: 'discord', label: 'Discord' },
];

export default function ScheduledTasks() {
  const { data: cronData, refetch } = useApi<{ jobs: CronJobInfo[]; total: number }>(() => api.cronJobs(), []);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [editingTask, setEditingTask] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [viewingLogs, setViewingLogs] = useState<string | null>(null);
  const [taskLogs, setTaskLogs] = useState<Array<{ id: number; task_id: string; timestamp: string; event_type: string; message: string }>>([]);
  const [logsLoading, setLogsLoading] = useState(false);

  // Create/Edit form state
  const [formId, setFormId] = useState('');
  const [formSchedule, setFormSchedule] = useState('@every 5m');
  const [formMessage, setFormMessage] = useState('');
  const [formIsAgent, setFormIsAgent] = useState(false);
  const [formChannel, setFormChannel] = useState('');
  const [formTarget, setFormTarget] = useState('');
  const [formSuppressKeyword, setFormSuppressKeyword] = useState('');

  const jobs = cronData?.jobs ?? [];

  const resetForm = () => {
    setFormId('');
    setFormSchedule('@every 5m');
    setFormMessage('');
    setFormIsAgent(false);
    setFormChannel('');
    setFormTarget('');
    setFormSuppressKeyword('');
    setEditingTask(null);
  };

  const openEdit = (job: CronJobInfo) => {
    const isAgent = job.message.startsWith('ask:');
    setFormId(job.id);
    setFormSchedule(job.schedule);
    setFormMessage(isAgent ? job.message.slice(4).trim() : job.message);
    setFormIsAgent(isAgent);
    setFormChannel(job.delivery?.channel || '');
    setFormTarget(job.delivery?.target || '');
    setFormSuppressKeyword(job.suppress_keyword || '');
    setEditingTask(job.id);
    setShowCreate(true);
  };

  const handleCreate = async () => {
    if (!formId.trim() || !formMessage.trim()) {
      setAlert({ type: 'error', message: 'Task ID and message are required.' });
      return;
    }

    setSaving(true);
    try {
      const message = formIsAgent ? `ask:${formMessage}` : formMessage;
      const delivery = formChannel ? { channel: formChannel, target: formTarget } : undefined;

      // If editing, delete the old task first
      if (editingTask) {
        await api.deleteScheduledTask(editingTask);
      }

      const res = await api.createScheduledTask({
        id: formId.trim(),
        schedule: formSchedule,
        message,
        delivery,
        suppress_keyword: formSuppressKeyword || undefined,
      });

      if (res.ok) {
        setAlert({ type: 'success', message: editingTask ? 'Task updated.' : 'Task created.' });
        setShowCreate(false);
        resetForm();
        refetch();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to save task.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    } finally {
      setSaving(false);
    }
  };

  const handleCancel = async (id: string) => {
    try {
      const res = await api.cancelScheduledTask(id);
      if (res.ok) {
        setAlert({ type: 'success', message: `Task '${id}' paused.` });
        refetch();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to pause.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    }
  };

  const handleResume = async (id: string) => {
    try {
      const res = await api.resumeScheduledTask(id);
      if (res.ok) {
        setAlert({ type: 'success', message: `Task '${id}' resumed.` });
        refetch();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to resume.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    }
  };

  const viewLogs = async (id: string) => {
    if (viewingLogs === id) {
      setViewingLogs(null);
      return;
    }
    setViewingLogs(id);
    setLogsLoading(true);
    try {
      const res = await api.scheduledTaskLogs(id);
      if (res.ok && res.data) {
        setTaskLogs(res.data.logs);
      }
    } catch {
      setTaskLogs([]);
    } finally {
      setLogsLoading(false);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      const res = await api.deleteScheduledTask(id);
      if (res.ok) {
        setAlert({ type: 'success', message: `Task '${id}' deleted.` });
        setConfirmDelete(null);
        refetch();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to delete.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    }
  };

  const parseMessageDisplay = (message: string) => {
    if (message.startsWith('ask:')) {
      return { type: 'Agent Prompt', text: message.slice(4).trim() };
    }
    return { type: 'Direct Message', text: message };
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-5">
        <h2 className="text-2xl font-semibold">Scheduled Tasks</h2>
        <button
          onClick={() => setShowCreate(true)}
          className="px-4 py-2 bg-[var(--color-accent)] text-white rounded-lg text-sm font-medium hover:bg-[var(--color-accent-hover)]"
        >
          + New Task
        </button>
      </div>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      {/* Task List */}
      {jobs.length > 0 ? (
        <div className="space-y-3">
          {jobs.map((job) => {
            const { type: msgType, text: msgText } = parseMessageDisplay(job.message);
            return (
              <div key={job.id} className="bg-white rounded-xl shadow-sm p-5">
                <div className="flex items-start justify-between">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 mb-1">
                      <span className="font-semibold text-gray-800">{job.id}</span>
                      <StatusBadge status={job.status} />
                      <span className={`text-xs px-2 py-0.5 rounded-full ${
                        msgType === 'Agent Prompt'
                          ? 'bg-purple-100 text-purple-700'
                          : 'bg-gray-100 text-gray-600'
                      }`}>
                        {msgType}
                      </span>
                    </div>
                    <div className="text-sm text-gray-600 mb-2 truncate">{msgText}</div>
                    <div className="flex items-center gap-4 text-xs text-gray-500">
                      <span className="flex items-center gap-1">
                        <span>🕐</span> {job.schedule}
                      </span>
                      {job.delivery && (
                        <span className="flex items-center gap-1">
                          <span>📡</span> {job.delivery.channel}
                          {job.delivery.target && ` → ${job.delivery.target}`}
                        </span>
                      )}
                    </div>
                    {job.last_error && (
                      <div className="mt-2 px-2.5 py-1.5 bg-red-50 border border-red-100 rounded-lg text-xs text-red-700 flex items-center gap-1.5">
                        <span>⚠️</span>
                        <span>{job.last_error.message}</span>
                        <span className="text-red-400 ml-auto shrink-0">{new Date(job.last_error.timestamp).toLocaleString()}</span>
                      </div>
                    )}
                  </div>
                  <div className="flex items-center gap-2 shrink-0 ml-4">
                    {job.status === 'Active' && (
                      <button
                        onClick={() => handleCancel(job.id)}
                        className="px-3 py-1.5 text-xs font-medium text-amber-700 bg-amber-50 rounded-lg hover:bg-amber-100"
                      >
                        Pause
                      </button>
                    )}
                    {job.status === 'Cancelled' && (
                      <button
                        onClick={() => handleResume(job.id)}
                        className="px-3 py-1.5 text-xs font-medium text-green-700 bg-green-50 rounded-lg hover:bg-green-100"
                      >
                        Resume
                      </button>
                    )}
                    <button
                      onClick={() => openEdit(job)}
                      className="px-3 py-1.5 text-xs font-medium text-blue-700 bg-blue-50 rounded-lg hover:bg-blue-100"
                    >
                      Edit
                    </button>
                    <button
                      onClick={() => viewLogs(job.id)}
                      className={`px-3 py-1.5 text-xs font-medium rounded-lg ${viewingLogs === job.id ? 'text-indigo-700 bg-indigo-100' : 'text-indigo-700 bg-indigo-50 hover:bg-indigo-100'}`}
                    >
                      Logs
                    </button>
                    <button
                      onClick={() => setConfirmDelete(job.id)}
                      className="px-3 py-1.5 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
                    >
                      Delete
                    </button>
                  </div>
                </div>

                {/* Logs Panel */}
                {viewingLogs === job.id && (
                  <div className="mt-3 pt-3 border-t border-gray-100">
                    {logsLoading ? (
                      <div className="text-xs text-gray-400 py-2">Loading logs...</div>
                    ) : taskLogs.length === 0 ? (
                      <div className="text-xs text-gray-400 py-2">No activity recorded yet.</div>
                    ) : (
                      <div className="space-y-1.5 max-h-64 overflow-y-auto">
                        {taskLogs.map((log) => (
                          <div key={log.id} className="flex items-start gap-2 text-xs">
                            <span className={`shrink-0 px-1.5 py-0.5 rounded font-medium ${
                              log.event_type === 'fired' ? 'bg-blue-100 text-blue-700' :
                              log.event_type === 'delivered' ? 'bg-green-100 text-green-700' :
                              log.event_type === 'skipped' ? 'bg-yellow-100 text-yellow-700' :
                              log.event_type === 'failed' ? 'bg-red-100 text-red-700' :
                              log.event_type === 'response' ? 'bg-purple-100 text-purple-700' :
                              'bg-gray-100 text-gray-600'
                            }`}>
                              {log.event_type}
                            </span>
                            <span className="text-gray-400 shrink-0">{new Date(log.timestamp).toLocaleTimeString()}</span>
                            <span className="text-gray-600 truncate">{log.message}</span>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ) : (
        <div className="text-center py-12 bg-white rounded-xl shadow-sm">
          <div className="text-4xl mb-3">📅</div>
          <h3 className="text-lg font-medium text-gray-700 mb-1">No scheduled tasks</h3>
          <p className="text-sm text-gray-500 mb-4">
            Create a task to run messages or agent prompts on a schedule.
          </p>
          <button
            onClick={() => setShowCreate(true)}
            className="px-4 py-2 bg-[var(--color-accent)] text-white rounded-lg text-sm font-medium hover:bg-[var(--color-accent-hover)]"
          >
            Create your first task
          </button>
        </div>
      )}

      {/* Create Task Modal */}
      {showCreate && (
        <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
          <div className="bg-white rounded-xl shadow-xl max-w-lg w-full p-6">
            <h3 className="text-lg font-semibold mb-4">{editingTask ? 'Edit Scheduled Task' : 'New Scheduled Task'}</h3>

            <div className="space-y-4">
              {/* Task ID */}
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Task ID</label>
                <input
                  type="text"
                  value={formId}
                  onChange={(e) => setFormId(e.target.value.replace(/[^a-zA-Z0-9_-]/g, ''))}
                  placeholder="daily-report"
                  disabled={!!editingTask}
                  className={`w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] ${editingTask ? 'bg-gray-100 text-gray-500' : ''}`}
                />
                {!editingTask && <p className="text-xs text-gray-400 mt-1">Letters, numbers, hyphens, underscores only.</p>}
              </div>

              {/* Schedule */}
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Schedule</label>
                <select
                  value={SCHEDULE_PRESETS.some(p => p.value === formSchedule) ? formSchedule : '__custom'}
                  onChange={(e) => {
                    if (e.target.value !== '__custom') setFormSchedule(e.target.value);
                  }}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] bg-white"
                >
                  {SCHEDULE_PRESETS.map((p) => (
                    <option key={p.value} value={p.value}>{p.label}</option>
                  ))}
                  <option value="__custom">Custom...</option>
                </select>
                {!SCHEDULE_PRESETS.some(p => p.value === formSchedule) && (
                  <input
                    type="text"
                    value={formSchedule}
                    onChange={(e) => setFormSchedule(e.target.value)}
                    placeholder="@every 10m or cron expression"
                    className="w-full mt-2 px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                  />
                )}
              </div>

              {/* Message Type Toggle */}
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-2">Task Type</label>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => setFormIsAgent(false)}
                    className={`flex-1 px-3 py-2 rounded-lg text-sm font-medium border-2 transition-all ${
                      !formIsAgent
                        ? 'border-[var(--color-accent)] bg-blue-50 text-blue-700'
                        : 'border-gray-200 text-gray-600 hover:border-gray-300'
                    }`}
                  >
                    💬 Direct Message
                  </button>
                  <button
                    type="button"
                    onClick={() => setFormIsAgent(true)}
                    className={`flex-1 px-3 py-2 rounded-lg text-sm font-medium border-2 transition-all ${
                      formIsAgent
                        ? 'border-purple-400 bg-purple-50 text-purple-700'
                        : 'border-gray-200 text-gray-600 hover:border-gray-300'
                    }`}
                  >
                    🤖 Agent Prompt
                  </button>
                </div>
                <p className="text-xs text-gray-400 mt-1">
                  {formIsAgent
                    ? 'The message will be sent to the agent as a prompt for processing.'
                    : 'The message will be delivered directly to the target channel.'}
                </p>
              </div>

              {/* Message */}
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">
                  {formIsAgent ? 'Agent Prompt' : 'Message'}
                </label>
                <textarea
                  value={formMessage}
                  onChange={(e) => setFormMessage(e.target.value)}
                  placeholder={formIsAgent ? 'Summarize today\'s news...' : 'Daily report ready!'}
                  rows={3}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] resize-none"
                />
              </div>

              {/* Delivery Target */}
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Deliver To (optional)</label>
                <div className="flex gap-2">
                  <select
                    value={formChannel}
                    onChange={(e) => setFormChannel(e.target.value)}
                    className="w-40 px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] bg-white"
                  >
                    {CHANNEL_OPTIONS.map((ch) => (
                      <option key={ch.value} value={ch.value}>{ch.label}</option>
                    ))}
                  </select>
                  {formChannel && (
                    <input
                      type="text"
                      value={formTarget}
                      onChange={(e) => setFormTarget(e.target.value)}
                      placeholder="chat ID or channel"
                      className="flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                    />
                  )}
                </div>
              </div>

              {/* Suppress Keyword */}
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Hide Response Keyword (optional)</label>
                <input
                  type="text"
                  value={formSuppressKeyword}
                  onChange={(e) => setFormSuppressKeyword(e.target.value)}
                  placeholder="e.g. HEARTBEAT_OK"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
                <p className="text-xs text-gray-400 mt-1">If the response contains only this keyword, it won't be sent to the user.</p>
              </div>
            </div>

            {/* Actions */}
            <div className="flex justify-end gap-3 mt-6">
              <button
                onClick={() => { setShowCreate(false); resetForm(); }}
                className="px-4 py-2 text-sm font-medium text-gray-600 hover:text-gray-800"
              >
                Cancel
              </button>
              <button
                onClick={handleCreate}
                disabled={saving || !formId.trim() || !formMessage.trim()}
                className="px-5 py-2 bg-[var(--color-accent)] text-white rounded-lg text-sm font-medium hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
              >
                {saving ? 'Saving...' : editingTask ? 'Update Task' : 'Create Task'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Delete Confirmation */}
      {confirmDelete && (
        <ConfirmDialog
          title="Delete Scheduled Task"
          message={`Are you sure you want to delete task "${confirmDelete}"? This cannot be undone.`}
          onConfirm={() => handleDelete(confirmDelete)}
          onCancel={() => setConfirmDelete(null)}
        />
      )}
    </div>
  );
}
