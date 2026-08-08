# CLI Hooks

`theway` can run user-configured hooks when agent lifecycle events fire. Hooks are best-effort
side effects: they can run shell commands or POST JSON to webhooks, but they do not mutate agent
state and failures do not fail the agent turn.

## Configuration

User hooks live at:

```text
~/.theway/hooks.toml
```

Project hooks can live at:

```text
<repo>/.theway/hooks.toml
```

Project hooks are ignored by default because they can execute commands from a cloned repository.
Enable them explicitly from your user config:

```toml
allow_project_hooks = true
```

or for one process:

```sh
THEWAY_ALLOW_PROJECT_HOOKS=1 theway
```

## Examples

Append every finished tool call to a log:

```toml
[[hook]]
event = "tool_end"
command = "echo \"$THEWAY_TOOL_NAME error=$THEWAY_TOOL_IS_ERROR\" >> ~/.theway/tool-hooks.log"
timeout_ms = 3000
```

Run only when the `bash` tool finishes:

```toml
[[hook]]
event = "tool_end"
tool = "bash"
command = "echo \"bash finished in $THEWAY_SESSION_ID\" >> ~/.theway/bash-hooks.log"
```

Send a webhook when a turn ends:

```toml
[[hook]]
event = "turn_end"
webhook = "https://example.com/theway/hooks"
timeout_ms = 5000

[hook.headers]
Authorization = "Bearer your-token"
```

Send a desktop notification on macOS when the agent finishes a response:

```toml
[[hook]]
event = "agent_end"
command = "osascript -e 'display notification \"theway finished\" with title \"theway\"'"
```

Send a webhook when context compaction runs:

```toml
[[hook]]
event = "compaction"
webhook = "https://example.com/theway/compaction"
timeout_ms = 5000
```

## Hook Fields

Each `[[hook]]` supports:

```toml
event = "tool_end"       # required
command = "..."          # optional shell command
webhook = "https://..."  # optional HTTP POST endpoint
timeout_ms = 5000        # optional, default 5000
enabled = true           # optional, default true
cwd = "project"          # project | theway | home, default project
on_failure = "warn"      # warn | ignore, default warn
tool = "bash"            # optional filter for tool_* events
```

`command` and `webhook` can be used together; the command runs first, then the webhook is sent.

## Events

Supported events:

```text
agent_start
agent_end
turn_start
turn_end
message_start
message_update
message_end
tool_start
tool_update
tool_end
compaction
```

`message_update` can fire frequently while a model streams. Use it only when you actually need
streaming-level callbacks.

`compaction` fires after successful automatic context compaction and after manual `/compact`.
Its payload includes `compaction_trigger = "auto" | "manual"`, the estimated summarized token
count, and a truncated summary. Compaction summaries can contain sensitive context; only send
them to destinations you trust.

## Command Environment

Command hooks receive environment variables:

```text
THEWAY_HOOK_EVENT
THEWAY_HOOK_PAYLOAD
THEWAY_SESSION_ID
THEWAY_CWD
THEWAY_MODEL_PROVIDER
THEWAY_MODEL_ID
THEWAY_THINKING_LEVEL
THEWAY_MESSAGE_KIND
THEWAY_ASSISTANT_EVENT
THEWAY_TOOL_CALL_ID
THEWAY_TOOL_NAME
THEWAY_TOOL_IS_ERROR
THEWAY_COMPACTION_TRIGGER
THEWAY_COMPACTION_TOKENS_BEFORE
```

`THEWAY_HOOK_PAYLOAD` points to a temporary JSON file containing the same payload sent to webhooks.
Compaction summaries are available in this JSON payload, not as an environment variable.

## Webhook Payload

Webhook hooks receive `Content-Type: application/json` with fields such as:

```json
{
  "event": "tool_end",
  "session_id": "...",
  "cwd": "/path/to/repo",
  "model_provider": "openai",
  "model_id": "gpt-5.5",
  "thinking_level": "off",
  "source": "user",
  "tool_call_id": "call_...",
  "tool_name": "bash",
  "tool_is_error": false,
  "tool_result_summary": "..."
}
```

Long message and tool summaries are truncated before being placed in the payload.

A compaction webhook payload looks like:

```json
{
  "event": "compaction",
  "session_id": "...",
  "cwd": "/path/to/repo",
  "model_provider": "openai",
  "model_id": "gpt-5.5",
  "thinking_level": "off",
  "source": "user",
  "compaction_trigger": "auto",
  "compaction_tokens_before": 12345,
  "compaction_summary": "..."
}
```

Long compaction summaries are truncated before being placed in the payload.
