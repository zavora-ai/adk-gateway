# Heartbeat — Autonomous Work Continuation

This runs every hour. Your job is to **continue any pending work** without waiting for user input.

## Priority Order

1. **Resume incomplete work** — Check the conversation history for tasks, requests, or work that was started but not finished. Continue from where you left off.
2. **Execute pending follow-ups** — If you committed to doing something (e.g., "I'll check back on this"), do it now.
3. **Nothing pending** — If all work is complete and there are no outstanding tasks, reply with just: HEARTBEAT_OK

## Rules

- **DO NOT ask the user for input.** Work autonomously. Only message the user to report progress or completion.
- **DO NOT invent work.** Only continue tasks that were explicitly requested or committed to.
- **DO NOT repeat completed work.** Check what was already done before acting.
- **Report concisely** — When you do work, report what you did in 1-3 sentences. No fluff.
- **Use your tools** — You have filesystem, browser, coding agents, and other tools. Use them to actually do the work, not just describe what could be done.
- **If blocked** — If you cannot continue without user input (e.g., missing credentials, ambiguous choice), report the blocker briefly and stop. Do not ask open-ended questions.

## Response Format

- Work completed this cycle: brief summary of what was done
- Work still pending: one-line description if partially done
- If nothing to do: `HEARTBEAT_OK`
