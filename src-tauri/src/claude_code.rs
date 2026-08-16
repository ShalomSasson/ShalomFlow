//! Claude Code CLI provider: fulfils chat requests by running the user's local
//! `claude` binary in headless print mode, on their own subscription login,
//! instead of an HTTP API or a downloaded GGUF model.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use log::{debug, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;

/// Model aliases offered for this provider. Kept in sync with
/// `CLAUDE_CODE_MODELS` in `AssistantSettings.tsx`.
pub const MODEL_ALIASES: [&str; 3] = ["sonnet", "opus", "haiku"];

/// One decoded line of the CLI's `stream-json` output.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StreamLine {
    /// Incremental assistant text. The only source of spoken tokens.
    TextDelta(String),
    /// The terminal `result` line for a successful run.
    Done { text: String },
    /// The terminal `result` line for a failed run.
    Failed { detail: String },
    /// Lifecycle events, rate-limit notices, the assembled assistant message,
    /// extended thinking, and anything unparseable.
    Ignored,
}

/// Classify one line of `stream-json`.
///
/// The CLI emits the reply twice: once as incremental
/// `stream_event`/`content_block_delta` chunks and again as a complete
/// `{"type":"assistant"}` message. Only the deltas yield tokens, otherwise
/// every reply would be duplicated. Unparseable lines are skipped rather than
/// treated as errors so a stray log line cannot kill a live reply.
pub(crate) fn parse_stream_line(line: &str) -> StreamLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return StreamLine::Ignored;
    }

    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        debug!("claude_code: skipping unparseable output line");
        return StreamLine::Ignored;
    };

    match value.get("type").and_then(Value::as_str) {
        Some("stream_event") => text_delta(value.get("event")),
        Some("result") => terminal_result(&value),
        _ => StreamLine::Ignored,
    }
}

/// Pull the text out of a `content_block_delta` event, if that is what this is.
fn text_delta(event: Option<&Value>) -> StreamLine {
    let Some(event) = event else {
        return StreamLine::Ignored;
    };
    if event.get("type").and_then(Value::as_str) != Some("content_block_delta") {
        return StreamLine::Ignored;
    }

    let delta = event.get("delta");
    // `thinking_delta` shares this envelope but is not part of the answer.
    if delta.and_then(|d| d.get("type")).and_then(Value::as_str) != Some("text_delta") {
        return StreamLine::Ignored;
    }

    match delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
        Some(text) if !text.is_empty() => StreamLine::TextDelta(text.to_string()),
        _ => StreamLine::Ignored,
    }
}

/// Interpret the terminal `result` line, which carries either the full reply or
/// the CLI's own error text.
fn terminal_result(value: &Value) -> StreamLine {
    let text = value
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if value
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let detail = if text.is_empty() {
            value
                .get("subtype")
                .and_then(Value::as_str)
                .unwrap_or("unknown Claude Code error")
                .to_string()
        } else {
            text
        };
        return StreamLine::Failed { detail };
    }

    StreamLine::Done { text }
}

/// Reduce `claude --version` output ("2.1.220 (Claude Code)") to just the
/// version so the UI can show it compactly.
pub(crate) fn parse_version(raw: &str) -> Option<String> {
    raw.split_whitespace().next().map(str::to_string)
}

/// First path in `candidates` that exists on disk.
fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.exists()).cloned()
}

/// Locate the user's `claude` binary.
///
/// A GUI app launched from Finder or Dock does not inherit the user's shell
/// `PATH`, so `Command::new("claude")` fails for most people even though the
/// command works in their terminal. Ask the login shell first (which honors
/// their own PATH customizations), then fall back to the standard install
/// locations.
pub(crate) fn resolve_binary() -> Result<PathBuf, String> {
    if let Some(path) = resolve_via_login_shell() {
        return Ok(path);
    }

    // Read HOME directly: the repo has no `dirs` crate and adding one for this
    // is not worth a new dependency.
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from);
    let mut candidates = Vec::new();
    if let Some(home) = home.as_ref() {
        candidates.push(home.join(".claude/local/claude"));
        candidates.push(home.join(".local/bin/claude"));
        candidates.push(home.join(".bun/bin/claude"));
        candidates.push(home.join(".npm-global/bin/claude"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    candidates.push(PathBuf::from("/usr/local/bin/claude"));

    first_existing(&candidates).ok_or_else(|| {
        "Claude Code isn't installed, or the `claude` command isn't on this machine's PATH."
            .to_string()
    })
}

/// Ask the user's login shell where `claude` lives.
fn resolve_via_login_shell() -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    // `command -v claude` is a fixed string; no user input is interpolated.
    let output = Command::new(shell)
        .arg("-lc")
        .arg("command -v claude")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
    (!path.as_os_str().is_empty() && path.exists()).then_some(path)
}

/// Extract plain text from a message `content`, which is either a string or an
/// array of typed parts. Non-text parts (images) are dropped: v1 is text-only.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Split OpenAI-style `messages` into a system prompt plus a single user
/// prompt.
///
/// The CLI's print mode takes one prompt and cannot be pre-seeded with
/// assistant turns, so any earlier conversation is replayed as labelled plain
/// text ahead of the live question.
pub(crate) fn flatten_messages(messages: &[Value]) -> (Option<String>, String) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut turns: Vec<(String, String)> = Vec::new();

    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_string();
        let text = content_text(message.get("content"));
        if text.trim().is_empty() {
            continue;
        }
        if role == "system" {
            system_parts.push(text);
        } else {
            turns.push((role, text));
        }
    }

    let system = (!system_parts.is_empty()).then(|| system_parts.join("\n\n"));

    // The live question is the final turn; everything before it is context.
    let live = turns.pop().map(|(_, text)| text).unwrap_or_default();
    if turns.is_empty() {
        return (system, live);
    }

    let mut prompt = String::from("Earlier in this conversation:\n");
    for (role, text) in &turns {
        let label = match role.as_str() {
            "assistant" => "Assistant",
            "tool" => "Tool result",
            _ => "User",
        };
        prompt.push_str(&format!("{label}: {text}\n"));
    }
    prompt.push_str("\nRespond to this message:\n");
    prompt.push_str(&live);
    (system, prompt)
}

/// Abort a stream that goes silent. Mirrors the HTTP path's SSE stall timeout
/// so a wedged CLI cannot hang the assistant forever.
const STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Safety net for a wedged one-shot call. Cleanup callers apply their own,
/// usually shorter, deadline on top of this.
const ONE_SHOT_TIMEOUT: Duration = Duration::from_secs(180);

/// Kills and reaps the child if the surrounding future is dropped (user hit
/// cancel, app quit). Without this, a cancelled turn leaks a `claude` process.
struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Build the argv shared by the streaming and one-shot paths.
///
/// Every value is passed as a separate argument — nothing is interpolated into
/// a shell string. `--tools ""` disables all built-in tools so the CLI acts as
/// a plain chat model and cannot touch the user's machine.
/// `--setting-sources ""` and `--disable-slash-commands` keep the user's
/// CLAUDE.md, skills, and hooks out of a voice reply, and
/// `--no-session-persistence` stops dictation transcripts accumulating on disk.
///
/// Deliberately absent: `--bare`. Per its own help text it forces
/// `ANTHROPIC_API_KEY`/`apiKeyHelper` auth and never reads OAuth or the
/// keychain — which would defeat the point of running on the user's login.
fn base_command(
    binary: &Path,
    model: &str,
    system_prompt: Option<&str>,
    output_format: &str,
    streaming: bool,
) -> Command {
    let mut cmd = Command::new(binary);
    cmd.arg("-p")
        .arg("--output-format")
        .arg(output_format)
        .arg("--model")
        .arg(model)
        .arg("--tools")
        .arg("")
        .arg("--setting-sources")
        .arg("")
        .arg("--disable-slash-commands")
        .arg("--no-session-persistence");

    if streaming {
        // Partial messages are what make token-by-token speech possible;
        // stream-json requires the verbose flag on some CLI versions.
        cmd.arg("--include-partial-messages").arg("--verbose");
    }

    if let Some(system) = system_prompt {
        cmd.arg("--system-prompt").arg(system);
    }

    // Run somewhere neutral so the CLI never picks up context from whatever
    // directory the app happened to be launched in.
    cmd.current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Send `prompt` to the child's stdin from its own thread.
///
/// A long flattened transcript can exceed the pipe buffer, so writing inline
/// before reading stdout would deadlock. Dropping stdin signals EOF, which is
/// what makes the CLI start work.
fn feed_stdin(stdin: std::process::ChildStdin, prompt: String) {
    std::thread::spawn(move || {
        let mut stdin = stdin;
        if let Err(e) = stdin.write_all(prompt.as_bytes()) {
            warn!("claude_code: failed to send prompt: {e}");
        }
    });
}

/// Drain a pipe to a String on its own thread, delivering it once complete.
fn collect_pipe<R: Read + Send + 'static>(pipe: R) -> tokio::sync::oneshot::Receiver<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut buffer = String::new();
        let mut pipe = pipe;
        let _ = pipe.read_to_string(&mut buffer);
        let _ = tx.send(buffer);
    });
    rx
}

/// Turn a raw CLI failure into something a user can act on.
pub(crate) fn describe_failure(detail: &str) -> String {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("login") || lower.contains("api key") || lower.contains("unauthorized") {
        return format!(
            "Claude Code isn't signed in. Run `claude` in a terminal once to log in, then try again. ({detail})"
        );
    }
    if lower.contains("rate limit") || lower.contains("usage limit") {
        return format!("Claude Code hit a usage limit on your plan. ({detail})");
    }
    format!("Claude Code failed: {detail}")
}

/// Stream a chat reply from the user's Claude Code CLI.
pub(crate) async fn stream_chat(
    model: &str,
    messages: &[Value],
    on_token: impl FnMut(&str),
) -> Result<String, String> {
    let binary = resolve_binary()?;
    stream_chat_with_binary(&binary, model, messages, on_token).await
}

/// Streaming implementation against an explicit binary path, so tests can point
/// at a stub instead of the real CLI.
async fn stream_chat_with_binary(
    binary: &Path,
    model: &str,
    messages: &[Value],
    mut on_token: impl FnMut(&str),
) -> Result<String, String> {
    let (system_prompt, prompt) = flatten_messages(messages);
    let mut child = base_command(binary, model, system_prompt.as_deref(), "stream-json", true)
        .spawn()
        .map_err(|e| format!("Couldn't start Claude Code: {e}"))?;

    let stdin = child.stdin.take().ok_or("Claude Code stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("Claude Code stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("Claude Code stderr unavailable")?;
    let mut guard = ChildGuard(Some(child));

    feed_stdin(stdin, prompt);
    let stderr_rx = collect_pipe(stderr);

    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut streamed = String::new();
    loop {
        match tokio::time::timeout(STALL_TIMEOUT, line_rx.recv()).await {
            Err(_) => {
                return Err(format!(
                    "Claude Code sent nothing for {}s — giving up on this reply.",
                    STALL_TIMEOUT.as_secs()
                ));
            }
            Ok(Some(line)) => match parse_stream_line(&line) {
                StreamLine::TextDelta(text) => {
                    on_token(&text);
                    streamed.push_str(&text);
                }
                StreamLine::Done { text } => {
                    // Prefer the streamed text (already spoken); fall back to
                    // the result field if no deltas arrived.
                    return Ok(if streamed.is_empty() { text } else { streamed });
                }
                StreamLine::Failed { detail } => return Err(describe_failure(&detail)),
                StreamLine::Ignored => {}
            },
            // Channel closed with no terminal result: the CLI died. Report the
            // exit status and stderr rather than a silently empty reply.
            Ok(None) => {
                if !streamed.is_empty() {
                    return Ok(streamed);
                }
                let code = guard
                    .0
                    .as_mut()
                    .and_then(|child| child.wait().ok())
                    .and_then(|status| status.code())
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let stderr = stderr_rx.await.unwrap_or_default();
                let detail = stderr.trim();
                return Err(describe_failure(&if detail.is_empty() {
                    format!("Claude Code exited with status {code} and no output.")
                } else {
                    detail.to_string()
                }));
            }
        }
    }
}

/// Run a single non-streaming completion (dictation cleanup, memory
/// distillation). Structured-output schemas are not supported by the CLI, which
/// is why this provider reports `supports_structured_output: false` — callers
/// pass a schema of `None` and parse leniently.
pub(crate) async fn complete(
    model: &str,
    user_content: &str,
    system_prompt: Option<&str>,
) -> Result<String, String> {
    let binary = resolve_binary()?;
    complete_with_binary(&binary, model, user_content, system_prompt).await
}

async fn complete_with_binary(
    binary: &Path,
    model: &str,
    user_content: &str,
    system_prompt: Option<&str>,
) -> Result<String, String> {
    let mut child = base_command(binary, model, system_prompt, "json", false)
        .spawn()
        .map_err(|e| format!("Couldn't start Claude Code: {e}"))?;

    let stdin = child.stdin.take().ok_or("Claude Code stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("Claude Code stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("Claude Code stderr unavailable")?;
    // Held for the whole call so a cancelled cleanup kills the child.
    let _guard = ChildGuard(Some(child));

    feed_stdin(stdin, user_content.to_string());
    let stdout_rx = collect_pipe(stdout);
    let stderr_rx = collect_pipe(stderr);

    let stdout = tokio::time::timeout(ONE_SHOT_TIMEOUT, stdout_rx)
        .await
        .map_err(|_| {
            format!(
                "Claude Code didn't answer within {}s.",
                ONE_SHOT_TIMEOUT.as_secs()
            )
        })?
        .map_err(|_| "Claude Code exited unexpectedly.".to_string())?;

    // With `--output-format json` the whole of stdout is one result object,
    // which is exactly what the stream parser already understands.
    match parse_stream_line(&stdout) {
        StreamLine::Done { text } => Ok(text),
        StreamLine::Failed { detail } => Err(describe_failure(&detail)),
        _ => {
            let stderr = stderr_rx.await.unwrap_or_default();
            let detail = stderr.trim();
            Err(describe_failure(&if detail.is_empty() {
                "Claude Code returned no usable output.".to_string()
            } else {
                detail.to_string()
            }))
        }
    }
}

/// What the settings UI needs to decide between offering this provider and
/// telling the user how to install it.
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct ClaudeCodeStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

/// Probe for the CLI. Cheap enough to call on settings mount; the version
/// shell-out only runs when a binary was actually found.
pub fn status() -> ClaudeCodeStatus {
    let Ok(path) = resolve_binary() else {
        return ClaudeCodeStatus {
            installed: false,
            path: None,
            version: None,
        };
    };

    let version = Command::new(&path)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| parse_version(&String::from_utf8_lossy(&out.stdout)));

    ClaudeCodeStatus {
        installed: true,
        path: Some(path.to_string_lossy().to_string()),
        version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim lines captured from `claude --output-format stream-json` (CLI 2.1.220).
    const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/tmp","session_id":"7d96","tools":[],"model":"claude-sonnet-5","apiKeySource":"none"}"#;
    const STATUS: &str = r#"{"type":"system","subtype":"status","status":"requesting","uuid":"ac36"}"#;
    const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"},"uuid":"2cfb"}"#;
    const DELTA_ONE: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello there"}},"uuid":"9a60"}"#;
    const DELTA_TWO: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":", friend!"}},"uuid":"fef4"}"#;
    const FULL_ASSISTANT_MESSAGE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello there, friend!"}]},"uuid":"0b9a"}"#;
    const BLOCK_START: &str = r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"uuid":"4973"}"#;
    const MESSAGE_STOP: &str = r#"{"type":"stream_event","event":{"type":"message_stop"},"uuid":"96f7"}"#;
    const RESULT_OK: &str = r#"{"is_error":false,"num_turns":1,"stop_reason":"end_turn","subtype":"success","result":"Hello there, friend!","type":"result","duration_ms":2860}"#;

    #[test]
    fn text_deltas_are_the_only_token_source() {
        assert_eq!(
            parse_stream_line(DELTA_ONE),
            StreamLine::TextDelta("Hello there".to_string())
        );
        assert_eq!(
            parse_stream_line(DELTA_TWO),
            StreamLine::TextDelta(", friend!".to_string())
        );

        // The CLI also emits the assembled message plus a pile of lifecycle
        // events. Treating any of them as tokens would duplicate the reply.
        for line in [
            INIT,
            STATUS,
            RATE_LIMIT,
            FULL_ASSISTANT_MESSAGE,
            BLOCK_START,
            MESSAGE_STOP,
        ] {
            assert_eq!(parse_stream_line(line), StreamLine::Ignored, "line: {line}");
        }
    }

    #[test]
    fn result_line_terminates_the_stream() {
        assert_eq!(
            parse_stream_line(RESULT_OK),
            StreamLine::Done {
                text: "Hello there, friend!".to_string()
            }
        );
    }

    #[test]
    fn error_results_surface_their_detail() {
        let auth_failure = r#"{"is_error":true,"subtype":"error_during_execution","result":"Invalid API key · Please run /login","type":"result"}"#;
        assert_eq!(
            parse_stream_line(auth_failure),
            StreamLine::Failed {
                detail: "Invalid API key · Please run /login".to_string()
            }
        );

        // An error with no message still has to report something actionable.
        let bare_failure = r#"{"is_error":true,"subtype":"error_max_turns","type":"result"}"#;
        assert_eq!(
            parse_stream_line(bare_failure),
            StreamLine::Failed {
                detail: "error_max_turns".to_string()
            }
        );
    }

    #[test]
    fn junk_and_thinking_lines_are_skipped() {
        // Non-JSON noise must never abort a reply mid-stream.
        assert_eq!(parse_stream_line(""), StreamLine::Ignored);
        assert_eq!(parse_stream_line("   "), StreamLine::Ignored);
        assert_eq!(parse_stream_line("not json at all"), StreamLine::Ignored);
        assert_eq!(parse_stream_line(r#"{"partial":"#), StreamLine::Ignored);

        // Extended thinking is not spoken output.
        let thinking = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#;
        assert_eq!(parse_stream_line(thinking), StreamLine::Ignored);
    }

    #[test]
    fn version_output_is_reduced_to_the_number() {
        assert_eq!(
            parse_version("2.1.220 (Claude Code)\n"),
            Some("2.1.220".to_string())
        );
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("   \n"), None);
    }

    #[test]
    fn discovery_prefers_the_first_existing_candidate() {
        let dir = std::env::temp_dir().join("speakoflow-claude-discovery-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let present = dir.join("claude");
        std::fs::write(&present, b"#!/bin/sh\n").expect("stub binary");
        let missing = dir.join("definitely-not-here");

        assert_eq!(
            first_existing(&[missing.clone(), present.clone()]),
            Some(present)
        );
        assert_eq!(first_existing(&[missing]), None);
        assert_eq!(first_existing(&[]), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_single_user_message_becomes_the_prompt_verbatim() {
        let messages = vec![
            serde_json::json!({ "role": "system", "content": "You are Iris, a terse voice assistant." }),
            serde_json::json!({ "role": "user", "content": "What is the capital of France?" }),
        ];

        let (system, prompt) = flatten_messages(&messages);
        assert_eq!(
            system.as_deref(),
            Some("You are Iris, a terse voice assistant.")
        );
        assert_eq!(prompt, "What is the capital of France?");
    }

    #[test]
    fn earlier_turns_are_replayed_as_a_labelled_transcript() {
        let messages = vec![
            serde_json::json!({ "role": "system", "content": "Be brief." }),
            serde_json::json!({ "role": "user", "content": "Remember the number 7." }),
            serde_json::json!({ "role": "assistant", "content": "Got it, 7." }),
            serde_json::json!({ "role": "user", "content": "What was the number?" }),
        ];

        let (system, prompt) = flatten_messages(&messages);
        assert_eq!(system.as_deref(), Some("Be brief."));
        // The live question must be last and clearly separated from the replay.
        assert!(prompt.ends_with("What was the number?"), "prompt: {prompt}");
        assert!(prompt.contains("User: Remember the number 7."));
        assert!(prompt.contains("Assistant: Got it, 7."));
    }

    #[test]
    fn image_parts_are_dropped_and_text_parts_kept() {
        // The vision path sends content as an array. v1 is text-only, so images
        // are ignored rather than crashing or leaking JSON into the prompt.
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "What is on my screen?" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }
            ]
        })];

        let (system, prompt) = flatten_messages(&messages);
        assert_eq!(system, None);
        assert_eq!(prompt, "What is on my screen?");
        assert!(!prompt.contains("base64"));
    }

    #[test]
    fn multiple_system_messages_are_joined() {
        let messages = vec![
            serde_json::json!({ "role": "system", "content": "Be brief." }),
            serde_json::json!({ "role": "system", "content": "Never use emoji." }),
            serde_json::json!({ "role": "user", "content": "Hi" }),
        ];

        let (system, prompt) = flatten_messages(&messages);
        assert_eq!(system.as_deref(), Some("Be brief.\n\nNever use emoji."));
        assert_eq!(prompt, "Hi");
    }

    /// Write an executable stub that mimics `claude` closely enough to exercise
    /// the spawn → read → parse path: it drains stdin, then replays fixture
    /// lines. No network, no tokens, no real CLI required.
    fn write_stub(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("speakoflow-claude-stub-{name}"));
        std::fs::create_dir_all(&dir).expect("stub dir");
        let path = dir.join("claude");
        std::fs::write(&path, body).expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
        }
        path
    }

    #[tokio::test]
    async fn streaming_emits_tokens_in_order_and_returns_the_full_reply() {
        let stub = write_stub(
            "ok",
            r#"#!/bin/sh
cat > /dev/null
printf '%s\n' '{"type":"system","subtype":"init","tools":[]}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Paris"}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" is the capital."}}}'
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Paris is the capital."}]}}'
printf '%s\n' '{"is_error":false,"subtype":"success","result":"Paris is the capital.","type":"result"}'
"#,
        );

        let mut tokens = Vec::new();
        let reply = stream_chat_with_binary(
            &stub,
            "sonnet",
            &[serde_json::json!({ "role": "user", "content": "Capital of France?" })],
            |token| tokens.push(token.to_string()),
        )
        .await
        .expect("stub stream should succeed");

        // Tokens arrive incrementally, and the duplicate assembled message must
        // not be counted a second time.
        assert_eq!(tokens, vec!["Paris", " is the capital."]);
        assert_eq!(reply, "Paris is the capital.");
    }

    #[tokio::test]
    async fn cli_reported_errors_become_err() {
        let stub = write_stub(
            "autherr",
            r#"#!/bin/sh
cat > /dev/null
printf '%s\n' '{"is_error":true,"subtype":"error_during_execution","result":"Invalid API key · Please run /login","type":"result"}'
"#,
        );

        let error = stream_chat_with_binary(
            &stub,
            "sonnet",
            &[serde_json::json!({ "role": "user", "content": "Hi" })],
            |_| {},
        )
        .await
        .expect_err("an is_error result must fail the call");

        assert!(error.contains("Invalid API key"), "error: {error}");
    }

    #[tokio::test]
    async fn a_crash_with_no_result_line_reports_stderr() {
        let stub = write_stub(
            "crash",
            r#"#!/bin/sh
cat > /dev/null
echo "claude: unrecognised option" >&2
exit 2
"#,
        );

        let error = stream_chat_with_binary(
            &stub,
            "sonnet",
            &[serde_json::json!({ "role": "user", "content": "Hi" })],
            |_| {},
        )
        .await
        .expect_err("a non-zero exit with no result must fail, not return empty text");

        assert!(
            error.contains("unrecognised option") || error.contains("exit"),
            "error should carry diagnostics, got: {error}"
        );
    }

    #[tokio::test]
    async fn a_large_prompt_does_not_deadlock_on_stdin() {
        // Well past the ~64KB pipe buffer: if the prompt were written inline
        // before reading stdout, this would hang instead of completing.
        let big = "word ".repeat(40_000);
        let stub = write_stub(
            "bigprompt",
            r#"#!/bin/sh
cat > /dev/null
printf '%s\n' '{"is_error":false,"subtype":"success","result":"ok","type":"result"}'
"#,
        );

        let reply = stream_chat_with_binary(
            &stub,
            "sonnet",
            &[serde_json::json!({ "role": "user", "content": big })],
            |_| {},
        )
        .await
        .expect("large prompts must stream without deadlocking");

        assert_eq!(reply, "ok");
    }

    #[tokio::test]
    async fn one_shot_completion_returns_the_result_field() {
        let stub = write_stub(
            "oneshot",
            r#"#!/bin/sh
cat > /dev/null
printf '%s\n' '{"is_error":false,"subtype":"success","result":"I went to the store today.","type":"result"}'
"#,
        );

        let cleaned = complete_with_binary(
            &stub,
            "sonnet",
            "um i went to the store today",
            Some("Clean up this transcript."),
        )
        .await
        .expect("stub completion should succeed");

        assert_eq!(cleaned, "I went to the store today.");
    }

    #[tokio::test]
    async fn one_shot_failures_are_reported() {
        let stub = write_stub(
            "oneshot-err",
            r#"#!/bin/sh
cat > /dev/null
printf '%s\n' '{"is_error":true,"subtype":"error_during_execution","result":"Please run /login","type":"result"}'
"#,
        );

        let error = complete_with_binary(&stub, "sonnet", "hello", None)
            .await
            .expect_err("an is_error result must fail the call");

        // The user needs to be told what to do, not just shown a raw subtype.
        assert!(error.contains("signed in"), "error: {error}");
    }

    #[test]
    fn status_reports_a_consistent_shape() {
        let status = status();
        // Whether or not the CLI is installed on this machine, the fields must
        // agree with each other — the UI branches on `installed`.
        if status.installed {
            assert!(status.path.is_some(), "installed status needs a path");
        } else {
            assert!(status.path.is_none(), "missing CLI must not report a path");
            assert!(status.version.is_none());
        }
    }
}
