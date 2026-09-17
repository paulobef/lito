//! ViewModel - UI state management and bridge to core agent

use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use lito_protocol::{ErrorCode, Operation, PlanStep};

use crate::generated::{
    AllowlistEntryData, ChatMessageData, CodeLineData, CodeSpanData, MainWindow, MessageBlockData,
    Palette, PlanStepData, SessionRowData,
};
use crate::highlight;
use crate::markdown;

/// UI State that mirrors what the Slint UI needs
#[derive(Debug, Clone, Default)]
pub struct UiState {
    pub is_processing: bool,
    pub ollama_connected: bool,
    pub ollama_model: String,
    pub available_models: Vec<String>,
    pub current_plan_id: String,
    /// Status of `current_plan_id`: `"streaming"` (the model is still
    /// writing it, so it has no id and no actions yet), `"pending"`
    /// (proposed, not run yet), `"done"` (executed), `"rejected"`, or empty
    /// if there's no plan.
    pub current_plan_status: String,
    /// The fs undo-log session id for the most recently executed plan - a
    /// fresh uuid minted per execution run by `FsOps`/`main.rs`, unrelated to
    /// `active_session_id` (the conversation this all happened in).
    pub current_undo_id: String,
    pub show_plan: bool,
    pub messages: VecDeque<ChatMessage>,
    pub plan_steps: Vec<PlanStepView>,
    /// The conversation currently loaded into the chat view.
    pub active_session_id: String,
    /// All sessions, most recently active first - unfiltered; the search
    /// query is applied client-side in `sync_to_ui`.
    pub sessions: Vec<SessionListItem>,
    pub session_search: String,
    pub dark_mode: bool,
    /// The Ollama endpoint currently in use, as last confirmed by a `Pong`
    /// (i.e. reflects what the core agent's `OllamaClient` actually has, not
    /// just what the settings field was last edited to).
    pub ollama_endpoint: String,
    /// `lito_llm::DEFAULT_BASE_URL`, pushed once at startup so the settings
    /// panel's "Réinitialiser" action doesn't need to hardcode a copy of it.
    pub default_ollama_endpoint: String,
    /// Command approval mode, as the persisted string ("validation" /
    /// "auto-restricted" / "auto-unsafe"). Confirmed by the core's
    /// `PolicyState`.
    pub approval_mode: String,
    /// The remembered-command allowlist, for the settings panel.
    pub allowlist_entries: Vec<AllowlistEntryView>,
    /// A command currently waiting on the user's decision, if any. Rendered as
    /// a card above the composer.
    pub pending_approval: Option<PendingApproval>,
    /// The Ollama connectivity diagnostic is running (probe in flight).
    pub diagnosing: bool,
    /// The finished diagnostic result, shown in the modal. `None` and
    /// `diagnosing == false` means the modal is closed.
    pub diagnosis: Option<DiagnosisView>,
}

/// A finished connectivity diagnostic, ready to render in the modal. Built by
/// `app` from `lito_llm::OllamaDiagnosis` (this crate doesn't depend on
/// `lito-llm`, so the strings arrive pre-formatted).
#[derive(Debug, Clone)]
pub struct DiagnosisView {
    pub ok: bool,
    pub needs_signin: bool,
    pub title: String,
    pub detail: String,
}

/// Chat message for UI display
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String, // "user", "assistant", "system"
    pub content: String,
    pub streaming: bool,
    /// This message's reasoning trace, if the model that produced it emitted
    /// one - `None` for user/system messages and for models that don't.
    pub thinking: Option<String>,
    /// Set only on the assistant message that just proposed a plan in this
    /// run (not reconstructed from persisted history), so the chat can show
    /// a "Plan proposed" chip that reopens the inspector panel.
    pub plan_id: Option<String>,
    pub plan_step_count: usize,
}

/// Plan step for UI display
#[derive(Debug, Clone, Default)]
pub struct PlanStepView {
    pub step: usize,
    pub total: usize,
    pub description: String,
    pub operation: String,
    pub status: String, // "pending", "running", "done", "error"
    /// RUN_COMMAND only: the argv joined for display, the working directory,
    /// whether it runs under a shell, and that it can't be undone.
    pub command: String,
    pub shell: bool,
    pub cwd: String,
    pub irreversible: bool,
    /// RUN_COMMAND only, set once `CommandOutput` arrives: whether the command
    /// actually ran, and if so its exit code (`None` if signalled) and output.
    pub command_ran: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
}

/// One row in the session sidebar list, as sent by the core. `group_label`
/// is the full group name ("Aujourd'hui", "Hier", "3 juin", ...) for every
/// row - `sync_to_ui` collapses consecutive rows sharing the same label into
/// a single header when building the Slint model, so grouping stays correct
/// even after the search filter removes some rows.
#[derive(Debug, Clone)]
pub struct SessionListItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub group_label: String,
}

/// The most recent plan proposed in a session, sent alongside
/// `CoreToUiMsg::SessionSwitched` so the inspector panel can be restored to
/// wherever that session left off.
#[derive(Debug, Clone)]
pub struct RestoredPlan {
    pub plan_id: String,
    pub steps: Vec<PlanStep>,
    pub status: String,
    pub undo_id: Option<String>,
}

/// What the user chose on a command approval card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Don't run it. For a plan step this fails the step and stops the plan.
    Deny,
    /// Run it this once.
    Allow,
    /// Run it and add `(argv, root)` to the auto-allow list. Ignored by the
    /// core if the request wasn't `rememberable` (i.e. a `shell: true` command).
    AllowAndRemember,
}

/// Where an approval request came from, so the UI can tell whether saying yes
/// resumes a plan or resumes plan generation.
#[derive(Debug, Clone)]
pub enum ApprovalOrigin {
    /// A `RUN_COMMAND` step in a plan being executed.
    PlanStep { step: usize, total: usize },
    /// A reconnaissance command the model asked to run mid-planning.
    Recon,
}

/// A command awaiting the user's decision, mirrored into `UiState` so the
/// approval card can render it above the composer.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Correlates the card's answer with the core's pending `oneshot`.
    pub request_id: String,
    pub argv: Vec<String>,
    pub shell: bool,
    pub cwd: String,
    /// `argv[0]` resolved to an absolute path, for the card's first line.
    pub resolved_program: String,
    /// Human-readable origin ("Étape 2/3", "Reconnaissance").
    pub origin_label: String,
    /// Whether to offer the "Autoriser et mémoriser" button.
    pub rememberable: bool,
}

/// One remembered-command entry, for the settings panel's revoke list.
#[derive(Debug, Clone)]
pub struct AllowlistEntryView {
    /// Opaque round-trip token (`serde_json` of the policy's `AllowlistKey`),
    /// sent straight back in `ForgetAllowlistEntry`.
    pub key: String,
    /// "ls -la  ·  ~/Documents"
    pub display: String,
}

/// Messages sent from UI to core agent
#[derive(Debug, Clone)]
pub enum UiToCoreMsg {
    SendMessage {
        prompt: String,
        context: String,
    },
    ExecutePlan {
        plan_id: String,
    },
    /// Undo everything done under a given fs undo-session id (see
    /// `UiState::current_undo_id`).
    Undo {
        session_id: String,
    },
    RejectPlan {
        plan_id: String,
    },
    NewSession,
    SwitchSession {
        session_id: String,
    },
    SetModel {
        model: String,
    },
    SetTheme {
        dark: bool,
    },
    /// Switch the Ollama endpoint the core agent's `OllamaClient` talks to.
    /// Handled like `SetModel`: the core updates the live client, persists
    /// the choice, then replies with a fresh `Pong` so the UI's connectivity
    /// state reflects the new endpoint rather than being set optimistically.
    SetEndpoint {
        url: String,
    },
    /// Stop the in-flight generation. Handled while `SendMessage` is still
    /// being awaited (see `run_core_agent`), which is the only time it can
    /// arrive - every other message queues behind it as usual.
    Cancel,
    /// The user's answer to a `CoreToUiMsg::ApprovalRequest`. Routed to the
    /// waiting `oneshot` by `drive_with_ui_pump` (see `app::main`).
    ApprovalResponse {
        request_id: String,
        decision: ApprovalDecision,
    },
    /// Switch the command approval mode ("validation" / "auto-restricted" /
    /// "auto-unsafe"). Persisted, then echoed back as `PolicyState`.
    SetApprovalMode {
        mode: String,
    },
    /// Drop one entry from the remembered-command allowlist. `key` is the
    /// opaque token from `AllowlistEntryView`.
    ForgetAllowlistEntry {
        key: String,
    },
    Ping,
    /// Run the Ollama connectivity diagnostic (see `OllamaClient::diagnose`):
    /// actually try the selected model, cloud models included, and reply with
    /// a `DiagnosisResult`. Requested from the settings panel, and also fired
    /// automatically by the core after a generation that errored out.
    DiagnoseOllama,
}

/// Messages sent from core agent to UI
#[derive(Debug, Clone)]
pub enum CoreToUiMsg {
    /// The active conversation changed - either the initial load at startup
    /// or the result of `NewSession`/`SwitchSession`. Carries everything
    /// needed to fully repaint the chat + plan inspector for that session.
    SessionSwitched {
        session_id: String,
        messages: Vec<(String, String, Option<String>)>,
        plan: Option<RestoredPlan>,
    },
    /// The full session list, sent after startup and after any mutation
    /// that could change titles/ordering/subtitles (new session, a message
    /// sent, a plan executed).
    SessionList {
        sessions: Vec<SessionListItem>,
    },
    Token {
        content: String,
    },
    /// A chunk of a reasoning-trained model's thinking trace, distinct from
    /// the answer/plan text carried by `Token`.
    Thinking {
        content: String,
    },
    /// The agent is running a LIST/STAT filesystem query before it can give
    /// a final answer - `description` is a short human-readable summary
    /// (e.g. "Listing ~/Desktop").
    Querying {
        description: String,
    },
    /// The plan steps parsed out of the response so far, while the model is
    /// still writing it - the full list each time, so the UI just replaces
    /// what it holds (see `lito_agent::AgentEvent::PlanSteps`). Superseded by
    /// `PlanReady` once the plan is complete and persisted.
    PlanStreaming {
        steps: Vec<PlanStep>,
    },
    MessageDone,
    /// Generation was stopped by the user. Distinct from `MessageDone`: the
    /// turn was never completed, so nothing about it was persisted.
    Cancelled,
    PlanReady {
        plan_id: String,
        steps: Vec<PlanStep>,
    },
    PlanRejected {
        plan_id: String,
    },
    StepStart {
        step: usize,
        total: usize,
        operation: Operation,
    },
    StepDone {
        step: usize,
        total: usize,
    },
    /// A step could not be completed (a policy refusal, a denied approval, a
    /// non-zero command exit, or a filesystem error). The plan stops here and
    /// its persisted status becomes `"failed"`.
    StepFailed {
        step: usize,
        total: usize,
        message: String,
    },
    /// The captured output of a `RUN_COMMAND` step that actually ran. Shown,
    /// truncated and folded, under the step in the plan panel.
    CommandOutput {
        step: usize,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        truncated: bool,
    },
    /// A command needs the user's approval before it can run. Answered with
    /// `UiToCoreMsg::ApprovalResponse` carrying the same `request_id`.
    ApprovalRequest {
        request_id: String,
        argv: Vec<String>,
        shell: bool,
        cwd: String,
        resolved_program: String,
        origin: ApprovalOrigin,
        rememberable: bool,
    },
    /// The current command-approval mode and remembered-command list, for the
    /// settings panel. Sent at startup and after any change.
    PolicyState {
        mode: String,
        allowlist: Vec<AllowlistEntryView>,
    },
    PlanDone {
        undo_id: String,
    },
    /// Undo finished. `irreversible_commands` is how many `RUN_COMMAND` steps
    /// in the run could not be undone (commands have no inverse).
    UndoDone {
        irreversible_commands: usize,
    },
    FileCreated {
        path: String,
    },
    FileMoved {
        from: String,
        to: String,
    },
    Pong {
        ollama: bool,
        model: String,
        available_models: Vec<String>,
        endpoint: String,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    /// Result of `UiToCoreMsg::DiagnoseOllama`. Strings are pre-formatted for
    /// display - the UI just drops them into the diagnostic modal.
    DiagnosisResult {
        /// The selected model actually served a token.
        ok: bool,
        /// The remedy is `ollama signin` - the modal then shows the sign-in
        /// instructions block.
        needs_signin: bool,
        /// Short heading, e.g. "Connexion opérationnelle".
        title: String,
        /// One or two sentences describing what was found.
        detail: String,
    },
}

/// Channel handles for UI <-> Core communication
pub struct ChannelHandles {
    pub ui_to_core_tx: mpsc::UnboundedSender<UiToCoreMsg>,
    pub core_to_ui_rx: mpsc::UnboundedReceiver<CoreToUiMsg>,
}

/// Owns the core agent task and hands out cloneable [`ViewModelHandle`]s.
pub struct ViewModel {
    handle: ViewModelHandle,
    agent_handle: JoinHandle<()>,
}

impl ViewModel {
    pub fn new(agent_handle: JoinHandle<()>) -> Self {
        Self {
            handle: ViewModelHandle::new(),
            agent_handle,
        }
    }

    /// Get a cloneable handle for wiring up the UI and core channels.
    pub fn handle(&self) -> ViewModelHandle {
        self.handle.clone()
    }

    /// Abort the core agent task (e.g. on window close).
    pub fn shutdown(&self) {
        self.agent_handle.abort();
    }
}

/// ViewModel handle for the UI
#[derive(Clone)]
pub struct ViewModelHandle {
    state: Arc<Mutex<UiState>>,
    ui_window: Arc<Mutex<Option<Weak<MainWindow>>>>,
    ui_to_core_tx: Arc<Mutex<Option<mpsc::UnboundedSender<UiToCoreMsg>>>>,
    /// Last (ollama_model, available_models) pushed to the model-selector
    /// `ComboBox`. `sync_to_ui` runs on every state change (e.g. every
    /// streamed token), but rebuilding+reassigning the `ComboBox`'s `model`
    /// on every one of those ticks - even to an equal list - makes it
    /// re-fire its internal `changed model` handler constantly, which resets
    /// `current-value` using whatever `current-index` happens to be mid-sync
    /// and can leave the selector showing blank/stale text (see std-widgets'
    /// `ComboBoxBase`). Only touching the selector's Slint properties when
    /// this cache shows they actually changed avoids that churn.
    model_selector_cache: Arc<Mutex<(String, Vec<String>)>>,
    /// `message_fingerprint` of every chat row as last pushed to Slint, so
    /// `sync_to_ui` can update just the rows that changed.
    ///
    /// Reassigning the whole `messages` model on every tick was the single
    /// biggest source of jank in the chat: a repeater compares the incoming
    /// `ModelRc` by pointer and, on any new instance, throws away every
    /// instantiated row *and* its `RepeaterLayoutState` - the scroll anchor,
    /// the running item-height average, the previous viewport position (see
    /// `i-slint-core`'s `Repeater::model`). At one reassignment per streamed
    /// token that means the chat lost its scroll anchoring tens of times a
    /// second, and any text selection or expanded reasoning block went with
    /// it, since the underlying `TextInput` instances were recreated too.
    message_fingerprints: Arc<Mutex<Vec<u64>>>,
    /// Whether `sync_to_ui` has installed its own `VecModel` into the
    /// window's `messages` property yet. Until it has, that property still
    /// holds the empty default from the `.slint` file, which is not a
    /// `VecModel` and so can't be mutated in place - the first sync assigns
    /// one, every later sync mutates it.
    ///
    /// The flag is flipped on the pump thread while the assignment happens
    /// later on the UI thread, which is safe only because
    /// `upgrade_in_event_loop` preserves ordering: the closure that installs
    /// the model is queued before any closure that expects to find it.
    messages_model_installed: Arc<AtomicBool>,
    /// Last `plan_steps_fingerprint` / `sessions_fingerprint` pushed, so
    /// those two models are reassigned only when their contents changed -
    /// same reasoning as `message_fingerprints`, but wholesale, since they
    /// change rarely enough not to need a row-level diff.
    plan_steps_cache: Arc<Mutex<Option<u64>>>,
    sessions_cache: Arc<Mutex<Option<u64>>>,
    /// Same guard, for the settings panel's remembered-command list.
    allowlist_cache: Arc<Mutex<Option<u64>>>,
}

/// Close out every message left marked `streaming` at the end of a turn, and
/// drop the ones that never received anything.
///
/// Not just the last one: a `Querying` system message lands *between* the
/// empty assistant placeholder `send_message` pushed and the tokens that
/// follow it, so the next `Token` starts a fresh assistant message and leaves
/// the placeholder orphaned mid-turn. It used to be invisible (empty content
/// renders nothing); since `ChatMessageRow` grew a caret keyed on
/// `streaming`, an orphan would blink "Génération…" forever.
fn finish_streaming(state: &mut UiState) {
    state.messages.retain(|m| {
        !(m.streaming && m.content.is_empty() && m.thinking.as_deref().unwrap_or("").is_empty())
    });
    for message in state.messages.iter_mut() {
        message.streaming = false;
    }
}

/// A filesystem-query / recon-command round just finished (a `Querying`
/// message is about to be appended). Settle the assistant message that drove
/// that round so that only the *current* assistant reply ever carries the
/// "Génération…" caret - without this, every round left its own streaming
/// message behind and the chat showed a stack of blinking indicators that
/// only cleared at end of turn. A round emits no answer tokens, so such a
/// message holds at most a reasoning trace: keep it (with `streaming` cleared,
/// which turns its auto-expanded "Réflexion…" block into a collapsed,
/// re-expandable one) or, if it received nothing at all, drop it.
fn settle_query_round(state: &mut UiState) {
    let Some(last) = state.messages.back_mut() else {
        return;
    };
    if last.role != "assistant" || !last.streaming {
        return;
    }
    if last.content.is_empty() && last.thinking.as_deref().unwrap_or("").is_empty() {
        state.messages.pop_back();
    } else {
        last.streaming = false;
    }
}

/// A reconnaissance-command / filesystem-query round has produced its result
/// (or the turn is otherwise moving on): drop the spinner on every `Querying`
/// system message. `finish_streaming` covers the end-of-turn cases; this is for
/// mid-turn, when the model resumes with reasoning, answer tokens or plan steps
/// while the `Querying` row is no longer the trailing message, so
/// `settle_query_round` (which only looks at a trailing *assistant* message)
/// never clears it. Without this the "Exécution en cours…" spinner would keep
/// turning under the command line for the rest of the turn.
fn settle_query_spinners(state: &mut UiState) {
    for message in state.messages.iter_mut() {
        if message.role == "system" && message.streaming {
            message.streaming = false;
        }
    }
}

/// Clear the plan panel if what it's showing is a plan the model was still
/// writing when the turn ended without producing one (`PlanStreaming` steps
/// that no `PlanReady` ever claimed). A finished, rejected or executed plan
/// is left alone.
fn discard_streaming_plan(state: &mut UiState) {
    if state.current_plan_status != "streaming" {
        return;
    }
    state.current_plan_status.clear();
    state.plan_steps.clear();
    state.show_plan = false;
}

/// Minimum wall-clock interval between two pushes of core-driven state into
/// the UI. ~30 Hz: fast enough that streamed text reads as continuous, slow
/// enough that a model emitting tokens far faster than that doesn't turn each
/// one into its own layout pass. Only the *push* is throttled - `UiState`
/// itself is always up to date. See `set_channels`.
const MIN_SYNC_INTERVAL: Duration = Duration::from_millis(33);

/// Everything about a chat message that the Slint row renders. Compared
/// against the previous sync's value to decide which rows to touch, so a
/// streamed token updates one row instead of replacing the whole model - and,
/// just as importantly, so `StyledText::from_markdown` runs only for the
/// message that actually changed rather than for the whole conversation on
/// every token.
fn message_fingerprint(m: &ChatMessage) -> u64 {
    let mut h = DefaultHasher::new();
    m.role.hash(&mut h);
    m.content.hash(&mut h);
    m.thinking.hash(&mut h);
    m.streaming.hash(&mut h);
    m.plan_id.hash(&mut h);
    m.plan_step_count.hash(&mut h);
    h.finish()
}

/// A chat row as built off the UI thread, before it becomes a
/// `ChatMessageData`.
///
/// The two differ in one field: a row's blocks are a `[MessageBlockData]` in
/// Slint, i.e. a `ModelRc`, and models are deliberately not `Send` - so a
/// finished `ChatMessageData` can't be moved into `upgrade_in_event_loop`'s
/// closure. Everything expensive (markdown parsing included) still happens on
/// the pump thread; only the `VecModel` wrapping happens on the UI thread,
/// via [`ChatRow::into_data`].
struct ChatRow {
    role: SharedString,
    content: SharedString,
    content_plain: SharedString,
    blocks: Vec<BlockRow>,
    thinking: SharedString,
    thinking_preview: SharedString,
    streaming: bool,
    plan_id: SharedString,
    plan_step_count: i32,
}

/// One block of `ChatRow`, mirroring `MessageBlockData` with one exception:
/// `lines` is plain highlighted-span data here (`highlight::highlight_code`'s
/// output, `Vec<Vec<highlight::Span>>`) rather than the `ModelRc<CodeLineData>`
/// `MessageBlockData.lines` needs - same reason `ChatRow.blocks` itself isn't
/// `Vec<MessageBlockData>` (see that doc above): a `ModelRc` field makes the
/// whole struct `!Send`, and this one is built on the pump thread.
struct BlockRow {
    kind: SharedString,
    content: SharedString,
    content_styled: slint::StyledText,
    language: SharedString,
    heading_level: i32,
    list_item: bool,
    code_lines: Vec<Vec<highlight::Span>>,
}

impl BlockRow {
    /// Finish the block. Must be called on the Slint UI thread, like
    /// `ChatRow::into_data` - this is where `lines`' `ModelRc`s come into
    /// existence.
    fn into_data(self) -> MessageBlockData {
        MessageBlockData {
            kind: self.kind,
            content: self.content,
            content_styled: self.content_styled,
            language: self.language,
            heading_level: self.heading_level,
            list_item: self.list_item,
            lines: code_lines_to_model(self.code_lines),
        }
    }
}

/// Build the `ModelRc<CodeLineData>` for a highlighted code block's `lines`.
/// UI-thread only (see `BlockRow::into_data`).
fn code_lines_to_model(lines: Vec<Vec<highlight::Span>>) -> ModelRc<CodeLineData> {
    ModelRc::new(VecModel::from(
        lines
            .into_iter()
            .map(|spans| CodeLineData {
                spans: code_spans_to_model(spans),
            })
            .collect::<Vec<_>>(),
    ))
}

/// Build the `ModelRc<CodeSpanData>` for one highlighted line or command.
/// UI-thread only, for the same reason as [`code_lines_to_model`].
fn code_spans_to_model(spans: Vec<highlight::Span>) -> ModelRc<CodeSpanData> {
    ModelRc::new(VecModel::from(
        spans
            .into_iter()
            .map(|s| CodeSpanData {
                text: s.text.into(),
                class: s.class.as_str().into(),
            })
            .collect::<Vec<_>>(),
    ))
}

/// First line of a reasoning trace, whitespace-collapsed and cut to a few
/// words, with a trailing ellipsis when anything was dropped. Shown in the
/// collapsed reasoning row (`thinking-preview` on `ChatMessageData`) so it
/// hints at what the model was reasoning about without expanding it.
fn thinking_preview(thinking: &str) -> String {
    const MAX_WORDS: usize = 10;
    let first_line = thinking
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let words: Vec<&str> = first_line.split_whitespace().collect();
    let more_lines = thinking.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    let dropped = words.len() > MAX_WORDS || more_lines;
    let mut preview = words
        .iter()
        .take(MAX_WORDS)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if dropped {
        preview.push('…');
    }
    preview
}

impl ChatRow {
    /// Finish the row. Must be called on the Slint UI thread, since this is
    /// where the (non-`Send`) block model comes into existence.
    fn into_data(self) -> ChatMessageData {
        ChatMessageData {
            role: self.role,
            content: self.content,
            content_plain: self.content_plain,
            blocks: ModelRc::new(VecModel::from(
                self.blocks
                    .into_iter()
                    .map(BlockRow::into_data)
                    .collect::<Vec<_>>(),
            )),
            thinking: self.thinking,
            thinking_preview: self.thinking_preview,
            streaming: self.streaming,
            plan_id: self.plan_id,
            plan_step_count: self.plan_step_count,
        }
    }
}

/// Build the Slint row for one chat message. Only called for rows whose
/// `message_fingerprint` changed.
fn chat_row(m: &ChatMessage) -> ChatRow {
    // Only assistant replies are model-generated markdown - system messages
    // are plain app-generated strings (a file path can legitimately contain a
    // stray "_" or "*") and would misrender if run through the CommonMark
    // parser. They get no blocks, which `ChatMessageRow` renders as one plain
    // `CopiableText` - see `blocks` on `ChatMessageData` in app-window.slint.
    let blocks = if m.role == "assistant" {
        markdown::split_blocks(&m.content)
    } else {
        Vec::new()
    };
    let content_plain = markdown::plain_text(&blocks);
    ChatRow {
        role: m.role.clone().into(),
        content: m.content.clone().into(),
        content_plain: content_plain.into(),
        blocks: blocks
            .into_iter()
            .map(|b| {
                // Only a fenced code block gets tokenized - see
                // `highlight.rs`'s module doc for why this exists at all
                // (`CopiableText`'s single uniform `color` can't show a
                // language's keywords/strings/comments differently).
                let code_lines = if b.kind == markdown::BlockKind::Code {
                    highlight::highlight_code(&b.content, &b.language)
                } else {
                    Vec::new()
                };
                BlockRow {
                    kind: b.kind.as_str().into(),
                    content: b.content.into(),
                    content_styled: b.styled.unwrap_or_default(),
                    language: b.language.into(),
                    heading_level: b.heading_level,
                    list_item: b.list_item,
                    code_lines,
                }
            })
            .collect(),
        thinking: m.thinking.clone().unwrap_or_default().into(),
        thinking_preview: thinking_preview(m.thinking.as_deref().unwrap_or_default()).into(),
        streaming: m.streaming,
        plan_id: m.plan_id.clone().unwrap_or_default().into(),
        plan_step_count: m.plan_step_count as i32,
    }
}

/// Build a `PlanStepView` from a persisted/streamed `PlanStep`, carrying the
/// `RUN_COMMAND` fields through. Output fields start empty; they're filled by
/// `CommandOutput` once the step runs.
fn plan_step_view(i: usize, total: usize, s: &PlanStep, status: &str) -> PlanStepView {
    PlanStepView {
        step: i + 1,
        total,
        description: s.description.clone(),
        operation: s.operation.to_string(),
        status: status.to_string(),
        command: s.argv.as_ref().map(|a| a.join(" ")).unwrap_or_default(),
        shell: s.shell,
        cwd: s
            .cwd
            .as_ref()
            .map(|c| c.display().to_string())
            .unwrap_or_default(),
        irreversible: s.operation == Operation::RunCommand,
        command_ran: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        output_truncated: false,
    }
}

/// Fingerprint of the whole plan-step list. Unlike the chat these change
/// rarely (once per executed step), so the model is replaced wholesale when
/// this differs rather than diffed row by row - the point is only to stop
/// reassigning it on every streamed token, which resets the `ListView`'s
/// layout state (see `messages_model_installed`).
fn plan_steps_fingerprint(steps: &[PlanStepView]) -> u64 {
    let mut h = DefaultHasher::new();
    for s in steps {
        s.step.hash(&mut h);
        s.total.hash(&mut h);
        s.description.hash(&mut h);
        s.operation.hash(&mut h);
        s.status.hash(&mut h);
        s.command.hash(&mut h);
        s.shell.hash(&mut h);
        s.cwd.hash(&mut h);
        s.irreversible.hash(&mut h);
        s.command_ran.hash(&mut h);
        s.exit_code.hash(&mut h);
        s.stdout.hash(&mut h);
        s.stderr.hash(&mut h);
        s.output_truncated.hash(&mut h);
    }
    h.finish()
}

/// A plan step as built off the UI thread, mirroring `PlanStepData` except
/// `command_spans` - highlighted-span data (`highlight::highlight_shell_command`'s
/// output) rather than the `ModelRc<CodeSpanData>` `PlanStepData.command-line`
/// needs. Same reason as `BlockRow`/`ChatRow`: a `ModelRc` field would make
/// `Vec<PlanStepRow>` `!Send`, and the whole `plan_steps` list is built on the
/// pump thread before being moved into `upgrade_in_event_loop`'s closure.
struct PlanStepRow {
    step: i32,
    total: i32,
    description: SharedString,
    operation: SharedString,
    status: SharedString,
    command: SharedString,
    command_spans: Vec<highlight::Span>,
    shell: bool,
    cwd: SharedString,
    irreversible: bool,
    command_ran: bool,
    exit_code: i32,
    stdout: SharedString,
    stderr: SharedString,
    output_truncated: bool,
}

impl PlanStepRow {
    /// Finish the step. Must be called on the Slint UI thread, like
    /// `BlockRow::into_data` - this is where `command-line`'s `ModelRc`
    /// comes into existence.
    fn into_data(self) -> PlanStepData {
        PlanStepData {
            step: self.step,
            total: self.total,
            description: self.description,
            operation: self.operation,
            status: self.status,
            command: self.command,
            command_line: code_spans_to_model(self.command_spans),
            shell: self.shell,
            cwd: self.cwd,
            irreversible: self.irreversible,
            command_ran: self.command_ran,
            exit_code: self.exit_code,
            stdout: self.stdout,
            stderr: self.stderr,
            output_truncated: self.output_truncated,
        }
    }
}

/// Fingerprint of the session list *as rendered* - the search query is part
/// of it, since filtering changes which rows and which group headers show.
fn sessions_fingerprint(sessions: &[SessionListItem], search: &str) -> u64 {
    let mut h = DefaultHasher::new();
    search.hash(&mut h);
    for s in sessions {
        s.id.hash(&mut h);
        s.title.hash(&mut h);
        s.subtitle.hash(&mut h);
        s.group_label.hash(&mut h);
    }
    h.finish()
}

impl ViewModelHandle {
    /// Create a new ViewModelHandle
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(UiState::default())),
            ui_window: Arc::new(Mutex::new(None)),
            ui_to_core_tx: Arc::new(Mutex::new(None)),
            model_selector_cache: Arc::new(Mutex::new((String::new(), Vec::new()))),
            message_fingerprints: Arc::new(Mutex::new(Vec::new())),
            messages_model_installed: Arc::new(AtomicBool::new(false)),
            plan_steps_cache: Arc::new(Mutex::new(None)),
            sessions_cache: Arc::new(Mutex::new(None)),
            allowlist_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the UI window weak reference for callbacks
    pub fn set_window(&self, window: Weak<MainWindow>) {
        *self.ui_window.lock().unwrap() = Some(window);
    }

    /// Store the outgoing channel and spawn a task that pumps incoming
    /// core messages into this handle for as long as the core is alive.
    ///
    /// The loop deliberately separates *applying* a message from *pushing*
    /// the result to the UI. A streaming reply arrives as one `Token` per
    /// few characters - far faster than a display can show them - and each
    /// push costs a marshalling hop onto the UI thread plus a layout pass
    /// over the chat. So every message is applied to `UiState` as soon as it
    /// lands (nothing is dropped or delayed in the model), everything
    /// already queued behind it is drained into the same batch, and only the
    /// push is rate-limited to `MIN_SYNC_INTERVAL`. The first message of a
    /// burst still syncs immediately, so a lone reply is not made to wait.
    pub fn set_channels(&self, channels: ChannelHandles) {
        *self.ui_to_core_tx.lock().unwrap() = Some(channels.ui_to_core_tx);

        let handle = self.clone();
        let mut core_to_ui_rx = channels.core_to_ui_rx;
        tokio::spawn(async move {
            let mut last_sync: Option<Instant> = None;
            while let Some(msg) = core_to_ui_rx.recv().await {
                handle.apply_core_message(msg);
                while let Ok(msg) = core_to_ui_rx.try_recv() {
                    handle.apply_core_message(msg);
                }

                if let Some(elapsed) = last_sync.map(|t| t.elapsed()) {
                    if elapsed < MIN_SYNC_INTERVAL {
                        tokio::time::sleep(MIN_SYNC_INTERVAL - elapsed).await;
                        // Anything that arrived while waiting joins this batch
                        // rather than triggering a second sync right after it.
                        while let Ok(msg) = core_to_ui_rx.try_recv() {
                            handle.apply_core_message(msg);
                        }
                    }
                }

                handle.sync_to_ui();
                last_sync = Some(Instant::now());
            }
        });
    }

    /// Update UI state with a closure, then push the result to the UI.
    /// Used by the public methods below, which are called straight from Slint
    /// callbacks on the UI thread and want their effect visible immediately.
    pub fn update_ui_state<F>(&self, f: F)
    where
        F: FnOnce(&mut UiState),
    {
        self.mutate_state(f);
        self.sync_to_ui();
    }

    /// Update UI state without pushing to the UI. See `apply_core_message`.
    fn mutate_state<F>(&self, f: F)
    where
        F: FnOnce(&mut UiState),
    {
        let mut state = self.state.lock().unwrap();
        f(&mut state);
    }

    /// Send a message to the core agent
    fn send_to_core(&self, msg: UiToCoreMsg) {
        if let Some(tx) = self.ui_to_core_tx.lock().unwrap().as_ref() {
            let _ = tx.send(msg);
        }
    }

    /// Apply an incoming core message to `UiState`. Deliberately does *not*
    /// push to the UI: the pump loop in `set_channels` drains everything
    /// already queued, then calls `sync_to_ui` once for the whole batch (see
    /// `MIN_SYNC_INTERVAL`).
    fn apply_core_message(&self, msg: CoreToUiMsg) {
        match msg {
            CoreToUiMsg::SessionSwitched {
                session_id,
                messages,
                plan,
            } => {
                self.mutate_state(|state| {
                    state.active_session_id = session_id;
                    state.messages.clear();
                    for (role, content, thinking) in messages {
                        state.messages.push_back(ChatMessage {
                            role,
                            content,
                            streaming: false,
                            thinking,
                            plan_id: None,
                            plan_step_count: 0,
                        });
                    }
                    state.current_plan_id.clear();
                    state.current_plan_status.clear();
                    state.current_undo_id.clear();
                    state.plan_steps.clear();
                    state.show_plan = false;
                    if let Some(plan) = plan {
                        let total = plan.steps.len();
                        let done = plan.status == "done";
                        let status = if done { "done" } else { "pending" };
                        state.plan_steps = plan
                            .steps
                            .iter()
                            .enumerate()
                            .map(|(i, s)| plan_step_view(i, total, s, status))
                            .collect();
                        state.current_plan_id = plan.plan_id;
                        state.current_plan_status = plan.status;
                        state.current_undo_id = plan.undo_id.unwrap_or_default();
                        state.show_plan = true;
                    }
                });
            }
            CoreToUiMsg::SessionList { sessions } => {
                self.mutate_state(|state| {
                    state.sessions = sessions;
                });
            }
            CoreToUiMsg::Token { content } => {
                self.mutate_state(|state| {
                    // The model has resumed emitting output - any recon /
                    // query round that was in flight has produced its result.
                    settle_query_spinners(state);
                    match state.messages.back_mut() {
                        Some(last) if last.role == "assistant" && last.streaming => {
                            last.content.push_str(&content);
                        }
                        _ => state.messages.push_back(ChatMessage {
                            role: "assistant".to_string(),
                            content,
                            streaming: true,
                            thinking: None,
                            plan_id: None,
                            plan_step_count: 0,
                        }),
                    }
                });
            }
            CoreToUiMsg::Thinking { content } => {
                self.mutate_state(|state| {
                    settle_query_spinners(state);
                    match state.messages.back_mut() {
                        Some(last) if last.role == "assistant" && last.streaming => {
                            last.thinking
                                .get_or_insert_with(String::new)
                                .push_str(&content);
                        }
                        _ => state.messages.push_back(ChatMessage {
                            role: "assistant".to_string(),
                            content: String::new(),
                            streaming: true,
                            thinking: Some(content),
                            plan_id: None,
                            plan_step_count: 0,
                        }),
                    }
                });
            }
            CoreToUiMsg::Querying { description } => {
                self.mutate_state(|state| {
                    settle_query_round(state);
                    // Any earlier recon / query round is finished by now.
                    settle_query_spinners(state);
                    state.messages.push_back(ChatMessage {
                        role: "system".to_string(),
                        // `streaming` on a system message is what drives the
                        // "Exécution en cours…" spinner in `ChatMessageRow` -
                        // the command / query is running and the model is
                        // blocked on its result. Cleared by
                        // `settle_query_spinners` (mid-turn) or
                        // `finish_streaming` (end of turn).
                        content: description,
                        streaming: true,
                        thinking: None,
                        plan_id: None,
                        plan_step_count: 0,
                    });
                });
            }
            CoreToUiMsg::PlanStreaming { steps } => {
                self.mutate_state(|state| {
                    settle_query_spinners(state);
                    // No plan id and no undo id yet - the plan isn't
                    // persisted until it's complete - so the panel shows the
                    // steps with neither the "Plan <id>" line nor the
                    // execute/reject actions. `"streaming"` is what
                    // app-window.slint keys both of those off.
                    state.current_plan_id.clear();
                    state.current_undo_id.clear();
                    state.current_plan_status = "streaming".to_string();
                    state.show_plan = true;
                    let total = steps.len();
                    state.plan_steps = steps
                        .iter()
                        .enumerate()
                        .map(|(i, s)| plan_step_view(i, total, s, "pending"))
                        .collect();
                });
            }
            CoreToUiMsg::MessageDone => {
                self.mutate_state(|state| {
                    state.is_processing = false;
                    finish_streaming(state);
                    // The response streamed steps but never parsed as a plan
                    // (see `StreamingPlanParser`) - drop the half-built panel
                    // rather than leaving steps on screen that no `PlanReady`
                    // will ever claim.
                    discard_streaming_plan(state);
                });
            }
            CoreToUiMsg::Cancelled => {
                self.mutate_state(|state| {
                    state.is_processing = false;
                    finish_streaming(state);
                    discard_streaming_plan(state);
                    state.messages.push_back(ChatMessage {
                        role: "system".to_string(),
                        content: "Génération interrompue.".to_string(),
                        streaming: false,
                        thinking: None,
                        plan_id: None,
                        plan_step_count: 0,
                    });
                });
            }
            CoreToUiMsg::PlanReady { plan_id, steps } => {
                self.mutate_state(|state| {
                    state.is_processing = false;
                    let step_count = steps.len();
                    // The streaming assistant message (if any) held the raw
                    // plan JSON as it arrived - replace it with a short note
                    // and attach the plan so the chat can show a chip that
                    // reopens the inspector panel.
                    if let Some(last) = state.messages.back_mut() {
                        if last.role == "assistant" && last.streaming {
                            last.content = "Voici un plan pour ça :".to_string();
                            last.streaming = false;
                            last.plan_id = Some(plan_id.clone());
                            last.plan_step_count = step_count;
                        }
                    }
                    finish_streaming(state);
                    state.current_plan_id = plan_id.clone();
                    state.current_plan_status = "pending".to_string();
                    state.show_plan = true;
                    state.plan_steps = steps
                        .iter()
                        .enumerate()
                        .map(|(i, s)| plan_step_view(i, step_count, s, "pending"))
                        .collect();
                });
            }
            CoreToUiMsg::PlanRejected { plan_id } => {
                self.mutate_state(|state| {
                    if state.current_plan_id == plan_id {
                        state.current_plan_status = "rejected".to_string();
                    }
                });
            }
            CoreToUiMsg::StepStart {
                step, operation, ..
            } => {
                self.mutate_state(|state| {
                    if let Some(s) = state.plan_steps.get_mut(step - 1) {
                        s.status = "running".to_string();
                        s.operation = operation.to_string();
                    }
                    state.is_processing = true;
                });
            }
            CoreToUiMsg::StepDone { step, total } => {
                self.mutate_state(|state| {
                    if let Some(s) = state.plan_steps.get_mut(step - 1) {
                        s.status = "done".to_string();
                    }
                    if step == total {
                        state.is_processing = false;
                    }
                });
            }
            CoreToUiMsg::PlanDone { undo_id } => {
                self.mutate_state(|state| {
                    state.current_undo_id = undo_id;
                    state.current_plan_status = "done".to_string();
                    state.is_processing = false;
                    state
                        .plan_steps
                        .iter_mut()
                        .for_each(|s| s.status = "done".to_string());
                    state.messages.push_back(ChatMessage {
                        role: "system".to_string(),
                        content: "Plan exécuté avec succès.".to_string(),
                        streaming: false,
                        thinking: None,
                        plan_id: None,
                        plan_step_count: 0,
                    });
                });
            }
            CoreToUiMsg::UndoDone {
                irreversible_commands,
            } => {
                self.mutate_state(|state| {
                    state.current_undo_id.clear();
                    if irreversible_commands > 0 {
                        state.messages.push_back(ChatMessage {
                            role: "system".to_string(),
                            content: format!(
                                "Annulation effectuée. {irreversible_commands} commande(s) \
                                 n'ont pas pu être annulées."
                            ),
                            streaming: false,
                            thinking: None,
                            plan_id: None,
                            plan_step_count: 0,
                        });
                    }
                });
            }
            CoreToUiMsg::StepFailed {
                step,
                total,
                message,
            } => {
                self.mutate_state(|state| {
                    if let Some(s) = state.plan_steps.get_mut(step - 1) {
                        s.status = "error".to_string();
                    }
                    state.is_processing = false;
                    state.current_plan_status = "failed".to_string();
                    state.pending_approval = None;
                    state.messages.push_back(ChatMessage {
                        role: "system".to_string(),
                        content: format!("Étape {step}/{total} : {message}"),
                        streaming: false,
                        thinking: None,
                        plan_id: None,
                        plan_step_count: 0,
                    });
                });
            }
            CoreToUiMsg::CommandOutput {
                step,
                exit_code,
                stdout,
                stderr,
                truncated,
            } => {
                self.mutate_state(|state| {
                    if let Some(s) = state.plan_steps.get_mut(step - 1) {
                        s.command_ran = true;
                        s.exit_code = exit_code;
                        s.stdout = stdout;
                        s.stderr = stderr;
                        s.output_truncated = truncated;
                    }
                });
            }
            CoreToUiMsg::ApprovalRequest {
                request_id,
                argv,
                shell,
                cwd,
                resolved_program,
                origin,
                rememberable,
            } => {
                self.mutate_state(|state| {
                    let origin_label = match origin {
                        ApprovalOrigin::PlanStep { step, total } => {
                            format!("Étape {step}/{total}")
                        }
                        ApprovalOrigin::Recon => "Reconnaissance".to_string(),
                    };
                    state.pending_approval = Some(PendingApproval {
                        request_id,
                        argv,
                        shell,
                        cwd,
                        resolved_program,
                        origin_label,
                        rememberable,
                    });
                });
            }
            CoreToUiMsg::PolicyState { mode, allowlist } => {
                self.mutate_state(|state| {
                    state.approval_mode = mode;
                    state.allowlist_entries = allowlist;
                });
            }
            CoreToUiMsg::Pong {
                ollama,
                model,
                available_models,
                endpoint,
            } => {
                self.mutate_state(|state| {
                    state.ollama_connected = ollama;
                    state.ollama_model = model;
                    state.available_models = available_models;
                    state.ollama_endpoint = endpoint;
                });
            }
            CoreToUiMsg::Error { code, message } => {
                self.mutate_state(|state| {
                    state.is_processing = false;
                    finish_streaming(state);
                    discard_streaming_plan(state);
                    state.messages.push_back(ChatMessage {
                        role: "system".to_string(),
                        content: format!("Error ({code:?}): {message}"),
                        streaming: false,
                        thinking: None,
                        plan_id: None,
                        plan_step_count: 0,
                    });
                });
            }
            CoreToUiMsg::DiagnosisResult {
                ok,
                needs_signin,
                title,
                detail,
            } => {
                self.mutate_state(|state| {
                    state.diagnosing = false;
                    state.diagnosis = Some(DiagnosisView {
                        ok,
                        needs_signin,
                        title,
                        detail,
                    });
                });
            }
            CoreToUiMsg::FileCreated { path } => {
                self.mutate_state(|state| {
                    state.messages.push_back(ChatMessage {
                        role: "system".to_string(),
                        content: format!("Created: {path}"),
                        streaming: false,
                        thinking: None,
                        plan_id: None,
                        plan_step_count: 0,
                    });
                });
            }
            CoreToUiMsg::FileMoved { from, to } => {
                self.mutate_state(|state| {
                    state.messages.push_back(ChatMessage {
                        role: "system".to_string(),
                        content: format!("Moved: {from} -> {to}"),
                        streaming: false,
                        thinking: None,
                        plan_id: None,
                        plan_step_count: 0,
                    });
                });
            }
        }
    }

    /// Sync state to UI. Safe to call from any thread: the actual property
    /// writes are marshalled onto the Slint UI thread.
    ///
    /// Everything is derived under a single `state` lock, which is released
    /// before the closure is queued. Each of the three list models is guarded
    /// by a cache so an unrelated tick (a streamed token, say) doesn't
    /// reassign it - see `message_fingerprints` for why that matters.
    fn sync_to_ui(&self) {
        let Some(window) = self.ui_window.lock().unwrap().clone() else {
            return;
        };
        let state = self.state.lock().unwrap();

        let is_processing = state.is_processing;
        let ollama_connected = state.ollama_connected;
        let current_plan_id = state.current_plan_id.clone();
        let current_plan_status = state.current_plan_status.clone();
        let current_undo_id = state.current_undo_id.clone();
        let show_plan = state.show_plan;
        let active_session_id = state.active_session_id.clone();
        let dark_mode = state.dark_mode;
        let ollama_endpoint = state.ollama_endpoint.clone();
        let default_ollama_endpoint = state.default_ollama_endpoint.clone();
        let approval_mode = state.approval_mode.clone();

        // -- Ollama diagnostic modal (scalars, pushed every sync) --
        let diagnosing = state.diagnosing;
        let (
            diagnosis_visible,
            diagnosis_ok,
            diagnosis_needs_signin,
            diagnosis_title,
            diagnosis_detail,
        ) = match &state.diagnosis {
            Some(d) => (
                true,
                d.ok,
                d.needs_signin,
                d.title.clone(),
                d.detail.clone(),
            ),
            None => (false, false, false, String::new(), String::new()),
        };

        // -- pending command approval (scalars, pushed every sync) --
        let (
            has_approval,
            approval_request_id,
            approval_argv,
            approval_argv_class,
            approval_shell,
            approval_cwd,
            approval_program,
            approval_origin,
            approval_rememberable,
        ) = match &state.pending_approval {
            Some(p) => (
                true,
                p.request_id.clone(),
                p.argv
                    .iter()
                    .map(|a| SharedString::from(a.as_str()))
                    .collect::<Vec<_>>(),
                p.argv
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        SharedString::from(highlight::classify_shell_word(a, i == 0).as_str())
                    })
                    .collect::<Vec<_>>(),
                p.shell,
                p.cwd.clone(),
                p.resolved_program.clone(),
                p.origin_label.clone(),
                p.rememberable,
            ),
            None => (
                false,
                String::new(),
                Vec::new(),
                Vec::new(),
                false,
                String::new(),
                String::new(),
                String::new(),
                false,
            ),
        };

        // -- remembered-command allowlist: replaced wholesale, only on change --
        let allowlist = {
            let mut h = DefaultHasher::new();
            for e in &state.allowlist_entries {
                e.key.hash(&mut h);
                e.display.hash(&mut h);
            }
            let fingerprint = h.finish();
            let mut cache = self.allowlist_cache.lock().unwrap();
            (*cache != Some(fingerprint)).then(|| {
                *cache = Some(fingerprint);
                state
                    .allowlist_entries
                    .iter()
                    .map(|e| AllowlistEntryData {
                        key: e.key.clone().into(),
                        display: e.display.clone().into(),
                    })
                    .collect::<Vec<_>>()
            })
        };

        // -- chat: row-level diff --
        // `install` is true exactly once, for the sync that puts our own
        // `VecModel` in place; it also forces every row to count as changed,
        // so `message_rows` covers the whole list that first time.
        let install_messages = !self.messages_model_installed.swap(true, Ordering::Relaxed);
        let message_count = state.messages.len();
        let message_rows: Vec<(usize, ChatRow)> = {
            let mut fingerprints = self.message_fingerprints.lock().unwrap();
            if install_messages {
                fingerprints.clear();
            }
            let new_fingerprints: Vec<u64> =
                state.messages.iter().map(message_fingerprint).collect();
            let rows = state
                .messages
                .iter()
                .enumerate()
                .filter(|(i, _)| fingerprints.get(*i) != Some(&new_fingerprints[*i]))
                .map(|(i, m)| (i, chat_row(m)))
                .collect();
            *fingerprints = new_fingerprints;
            rows
        };

        // -- plan steps: replaced wholesale, but only when they changed --
        let plan_steps = {
            let fingerprint = plan_steps_fingerprint(&state.plan_steps);
            let mut cache = self.plan_steps_cache.lock().unwrap();
            (*cache != Some(fingerprint)).then(|| {
                *cache = Some(fingerprint);
                state
                    .plan_steps
                    .iter()
                    .map(|s| PlanStepRow {
                        step: s.step as i32,
                        total: s.total as i32,
                        description: s.description.clone().into(),
                        operation: s.operation.clone().into(),
                        status: s.status.clone().into(),
                        command_spans: if s.command.is_empty() {
                            Vec::new()
                        } else {
                            highlight::highlight_shell_command(&s.command)
                        },
                        command: s.command.clone().into(),
                        shell: s.shell,
                        cwd: s.cwd.clone().into(),
                        irreversible: s.irreversible,
                        command_ran: s.command_ran,
                        exit_code: s.exit_code.unwrap_or(-1),
                        stdout: s.stdout.clone().into(),
                        stderr: s.stderr.clone().into(),
                        output_truncated: s.output_truncated,
                    })
                    .collect::<Vec<_>>()
            })
        };

        // Only touch the ComboBox's `model`/`current-index` when the
        // selector's data actually changed (see `model_selector_cache`'s
        // doc) - otherwise every unrelated state update (e.g. a streamed
        // token) would reassign them to an equal-but-new list and spuriously
        // reset the selector's displayed text.
        let selector_changed = {
            let mut cache = self.model_selector_cache.lock().unwrap();
            if cache.0 != state.ollama_model || cache.1 != state.available_models {
                *cache = (state.ollama_model.clone(), state.available_models.clone());
                true
            } else {
                false
            }
        };
        let selector = selector_changed.then(|| {
            let available_models: Vec<slint::SharedString> = state
                .available_models
                .iter()
                .map(|m| m.clone().into())
                .collect();
            let current_model_index = state
                .available_models
                .iter()
                .position(|m| m == &state.ollama_model)
                .unwrap_or(0) as i32;
            (
                state.ollama_model.clone(),
                available_models,
                current_model_index,
            )
        });

        // -- sessions: filter by the search query and collapse consecutive
        // rows sharing the same group into a single header, so grouping stays
        // correct even once filtering removes some rows from a group. Also
        // cached, for the same reason as the plan steps.
        let sessions = {
            let fingerprint = sessions_fingerprint(&state.sessions, &state.session_search);
            let mut cache = self.sessions_cache.lock().unwrap();
            (*cache != Some(fingerprint)).then(|| {
                *cache = Some(fingerprint);
                let search = state.session_search.to_lowercase();
                let mut last_group = String::new();
                state
                    .sessions
                    .iter()
                    .filter(|s| search.is_empty() || s.title.to_lowercase().contains(&search))
                    .map(|s| {
                        let header = if s.group_label != last_group {
                            last_group = s.group_label.clone();
                            s.group_label.clone()
                        } else {
                            String::new()
                        };
                        SessionRowData {
                            id: s.id.clone().into(),
                            title: s.title.clone().into(),
                            subtitle: s.subtitle.clone().into(),
                            group_label: header.into(),
                        }
                    })
                    .collect::<Vec<_>>()
            })
        };

        drop(state);

        let _ = window.upgrade_in_event_loop(move |window| {
            window.set_is_processing(is_processing);
            window.set_ollama_connected(ollama_connected);
            if let Some((ollama_model, available_models, current_model_index)) = selector {
                window.set_ollama_model(ollama_model.into());
                window.set_available_models(ModelRc::new(VecModel::from(available_models)));
                window.set_current_model_index(current_model_index);
            }
            window.set_current_plan_id(current_plan_id.into());
            window.set_current_plan_status(current_plan_status.into());
            window.set_current_undo_id(current_undo_id.into());
            window.set_show_plan(show_plan);
            window.set_active_session_id(active_session_id.into());
            window.set_ollama_endpoint(ollama_endpoint.into());
            window.set_default_ollama_endpoint(default_ollama_endpoint.into());
            window.set_approval_mode(approval_mode.into());
            window.set_has_approval(has_approval);
            window.set_approval_request_id(approval_request_id.into());
            window.set_approval_argv(ModelRc::new(VecModel::from(approval_argv)));
            window.set_approval_argv_class(ModelRc::new(VecModel::from(approval_argv_class)));
            window.set_approval_shell(approval_shell);
            window.set_approval_cwd(approval_cwd.into());
            window.set_approval_program(approval_program.into());
            window.set_approval_origin(approval_origin.into());
            window.set_approval_rememberable(approval_rememberable);
            window.set_diagnosing(diagnosing);
            window.set_diagnosis_visible(diagnosis_visible || diagnosing);
            window.set_diagnosis_ok(diagnosis_ok);
            window.set_diagnosis_needs_signin(diagnosis_needs_signin);
            window.set_diagnosis_title(diagnosis_title.into());
            window.set_diagnosis_detail(diagnosis_detail.into());
            if let Some(allowlist) = allowlist {
                window.set_command_allowlist(ModelRc::new(VecModel::from(allowlist)));
            }
            if let Some(plan_steps) = plan_steps {
                window.set_plan_steps(ModelRc::new(VecModel::from(
                    plan_steps
                        .into_iter()
                        .map(PlanStepRow::into_data)
                        .collect::<Vec<_>>(),
                )));
            }
            if let Some(sessions) = sessions {
                window.set_sessions(ModelRc::new(VecModel::from(sessions)));
            }

            if install_messages {
                window.set_messages(ModelRc::new(VecModel::from(
                    message_rows
                        .into_iter()
                        .map(|(_, row)| row.into_data())
                        .collect::<Vec<_>>(),
                )));
            } else {
                let model = window.get_messages();
                match model.as_any().downcast_ref::<VecModel<ChatMessageData>>() {
                    Some(vec_model) => {
                        // Shrink first (a session switch can leave fewer rows
                        // than the model holds), then apply the changed rows.
                        // They arrive in ascending index order, and every
                        // index past the old end is by definition "changed",
                        // so appends land as pushes in the right order.
                        while vec_model.row_count() > message_count {
                            vec_model.remove(vec_model.row_count() - 1);
                        }
                        for (i, row) in message_rows {
                            let row = row.into_data();
                            if i < vec_model.row_count() {
                                vec_model.set_row_data(i, row);
                            } else {
                                vec_model.push(row);
                            }
                        }
                    }
                    None => {
                        // Can't happen unless something else assigns to
                        // `messages`; loud rather than a silently frozen chat.
                        tracing::error!(
                            "chat model is not the VecModel sync_to_ui installed - \
                             {} row update(s) dropped",
                            message_rows.len()
                        );
                    }
                }
            }

            window.global::<Palette>().set_dark(dark_mode);
        });
    }

    /// Public API methods, called directly from UI callbacks (already on the
    /// UI thread) so these are plain synchronous calls.
    pub fn send_message(&self, prompt: String, context: String) {
        if prompt.trim().is_empty() {
            return;
        }
        self.update_ui_state(|state| {
            state.is_processing = true;
            state.messages.push_back(ChatMessage {
                role: "user".to_string(),
                content: prompt.clone(),
                streaming: false,
                thinking: None,
                plan_id: None,
                plan_step_count: 0,
            });
            state.messages.push_back(ChatMessage {
                role: "assistant".to_string(),
                content: String::new(),
                streaming: true,
                thinking: None,
                plan_id: None,
                plan_step_count: 0,
            });
        });
        self.send_to_core(UiToCoreMsg::SendMessage { prompt, context });
    }

    /// Stop the in-flight generation. Doesn't touch local state: the core
    /// replies with `Cancelled`, which is what clears `is_processing` and
    /// closes out the streaming message - so a cancel that races a response
    /// that already finished can't leave the UI wedged mid-stream.
    pub fn cancel(&self) {
        self.send_to_core(UiToCoreMsg::Cancel);
    }

    /// Answer the pending command approval card. `decision` is `"allow"`,
    /// `"remember"`, or anything else (treated as deny). Clears the card
    /// locally so it disappears at once, then routes the answer to the core's
    /// waiting `oneshot`.
    pub fn respond_to_approval(&self, request_id: String, decision: String) {
        let decision = match decision.as_str() {
            "allow" => ApprovalDecision::Allow,
            "remember" => ApprovalDecision::AllowAndRemember,
            _ => ApprovalDecision::Deny,
        };
        self.update_ui_state(|state| {
            if state
                .pending_approval
                .as_ref()
                .is_some_and(|p| p.request_id == request_id)
            {
                state.pending_approval = None;
            }
        });
        self.send_to_core(UiToCoreMsg::ApprovalResponse {
            request_id,
            decision,
        });
    }

    /// Switch the command approval mode. The core persists it and echoes a
    /// `PolicyState` back, which is what actually updates `UiState`.
    pub fn set_approval_mode(&self, mode: String) {
        self.send_to_core(UiToCoreMsg::SetApprovalMode { mode });
    }

    /// Revoke one remembered command (the settings panel's per-row "×").
    pub fn forget_allowlist_entry(&self, key: String) {
        self.send_to_core(UiToCoreMsg::ForgetAllowlistEntry { key });
    }

    pub fn execute_plan(&self, plan_id: String) {
        self.update_ui_state(|state| {
            state.is_processing = true;
        });
        self.send_to_core(UiToCoreMsg::ExecutePlan { plan_id });
    }

    pub fn undo(&self, session_id: String) {
        self.send_to_core(UiToCoreMsg::Undo { session_id });
    }

    pub fn reject_plan(&self, plan_id: String) {
        self.send_to_core(UiToCoreMsg::RejectPlan { plan_id });
    }

    /// Create a new conversation. Clears the chat/plan view immediately for
    /// a snappy UI; the core's `SessionSwitched` reply (empty history, no
    /// plan) will confirm the same state once the session is persisted.
    pub fn new_session(&self) {
        self.update_ui_state(|state| {
            state.messages.clear();
            state.show_plan = false;
            state.plan_steps.clear();
            state.current_plan_id.clear();
            state.current_plan_status.clear();
            state.current_undo_id.clear();
        });
        self.send_to_core(UiToCoreMsg::NewSession);
    }

    /// Switch to a different conversation. Optimistically highlights the
    /// selected row; the core's `SessionSwitched` reply repaints the chat.
    pub fn select_session(&self, session_id: String) {
        self.update_ui_state(|state| {
            state.active_session_id = session_id.clone();
        });
        self.send_to_core(UiToCoreMsg::SwitchSession { session_id });
    }

    /// Filter the session list by title, entirely client-side against the
    /// last list the core sent - no round trip needed.
    pub fn search_sessions(&self, text: String) {
        self.update_ui_state(|state| {
            state.session_search = text;
        });
    }

    /// Reopen the plan inspector panel for the currently loaded plan (e.g.
    /// from the chat's "Plan proposed" chip). Purely local: the panel's data
    /// is already in `UiState`, no core round trip needed.
    pub fn open_plan(&self) {
        self.update_ui_state(|state| {
            state.show_plan = true;
        });
    }

    /// Close the plan inspector panel. Purely local, see `open_plan`.
    pub fn close_plan(&self) {
        self.update_ui_state(|state| {
            state.show_plan = false;
        });
    }

    pub fn set_model(&self, model: String) {
        self.send_to_core(UiToCoreMsg::SetModel { model });
    }

    /// Switch the Ollama endpoint. Not applied optimistically - like
    /// `set_model`, `UiState::ollama_endpoint` only updates once the core's
    /// `Pong` reply confirms the new client actually works, so a bad URL
    /// shows up as "hors ligne" rather than a silently accepted typo.
    pub fn set_endpoint(&self, url: String) {
        self.send_to_core(UiToCoreMsg::SetEndpoint { url });
    }

    /// Toggle light/dark theme. Applied optimistically to the local state
    /// (and hence immediately to the UI); `SetTheme` just asks the core to
    /// persist the choice for next launch.
    pub fn toggle_theme(&self, dark: bool) {
        self.update_ui_state(|state| {
            state.dark_mode = dark;
        });
        self.send_to_core(UiToCoreMsg::SetTheme { dark });
    }

    pub fn ping(&self) {
        self.send_to_core(UiToCoreMsg::Ping);
    }

    /// Run the Ollama connectivity diagnostic and open the result modal. The
    /// modal shows a spinner-less "in progress" state until the core replies
    /// with `DiagnosisResult` (bounded to ~20s by `OllamaClient::diagnose`).
    pub fn diagnose_ollama(&self) {
        self.update_ui_state(|state| {
            state.diagnosing = true;
            state.diagnosis = None;
        });
        self.send_to_core(UiToCoreMsg::DiagnoseOllama);
    }

    /// Close the diagnostic modal.
    pub fn dismiss_diagnosis(&self) {
        self.update_ui_state(|state| {
            state.diagnosing = false;
            state.diagnosis = None;
        });
    }

    /// Copy a chat message's raw text to the system clipboard (the
    /// "Copier" action on `ChatMessageRow`). Purely local - no core
    /// round-trip. `ClipboardContext::new` opens a fresh platform clipboard
    /// handle per call rather than keeping one around on `ViewModelHandle`;
    /// this is an infrequent, user-initiated action, not a hot path, so the
    /// extra open cost isn't worth the complexity of caching a handle that
    /// isn't `Sync` on every backend.
    pub fn copy_message(&self, text: String) {
        use copypasta::ClipboardProvider;
        match copypasta::ClipboardContext::new() {
            Ok(mut ctx) => {
                if let Err(e) = ctx.set_contents(text) {
                    tracing::warn!("failed to copy message to clipboard: {e}");
                }
            }
            Err(e) => tracing::warn!("failed to open system clipboard: {e}"),
        }
    }
}

impl Default for ViewModelHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::thinking_preview;

    #[test]
    fn preview_empty_stays_empty() {
        assert_eq!(thinking_preview(""), "");
        assert_eq!(thinking_preview("   \n  \n"), "");
    }

    #[test]
    fn preview_short_single_line_is_verbatim() {
        assert_eq!(thinking_preview("Just three words"), "Just three words");
    }

    #[test]
    fn preview_caps_at_ten_words_with_ellipsis() {
        let long = "one two three four five six seven eight nine ten eleven twelve";
        assert_eq!(
            thinking_preview(long),
            "one two three four five six seven eight nine ten…"
        );
    }

    #[test]
    fn preview_uses_first_non_empty_line_and_marks_more() {
        assert_eq!(
            thinking_preview("\n\nFirst real line here\nsecond line"),
            "First real line here…"
        );
    }

    #[test]
    fn preview_collapses_inner_whitespace() {
        assert_eq!(thinking_preview("a\t\t b   c"), "a b c");
    }
}
