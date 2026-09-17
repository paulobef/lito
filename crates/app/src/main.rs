//! Lito App - Main entry point
//!
//! Single binary, single process, Rust + Slint UI
//! Architecture: UI (Slint) <-> Core Agent (async channels) <-> Services (FS, LLM, Memory)

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use tokio::sync::{mpsc, oneshot};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use lito_agent::{Agent, AgentEvent, AgentResponse};
use lito_exec::CommandOutcome;
use lito_fs::FsOps;
use lito_llm::{ChatMessage, OllamaClient, OllamaDiagnosis};
use lito_memory::MemoryStore;
use lito_policy::{AllowlistKey, ApprovalMode, CommandPolicy, CommandSpec, Verdict};
use lito_protocol::{ErrorCode, Operation, PlanId, PlanStep};
use lito_ui::{
    AllowlistEntryView, ApprovalDecision, ApprovalOrigin, CoreToUiMsg, RestoredPlan,
    SessionListItem, UiToCoreMsg, ViewModel,
};

/// How many past turns to keep in memory and send to the model as
/// conversation context. Bounds prompt size/latency; older turns stay in
/// SQLite but drop out of what the model sees.
const MAX_HISTORY_MESSAGES: usize = 20;

/// French month names for session-list date group headers older than
/// yesterday (e.g. "3 juin") - hardcoded rather than pulling in a locale
/// dependency, consistent with the rest of this app's hardcoded French UI
/// strings.
const FRENCH_MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// Installs a winit backend whose windows use a transparent, full-size-content
/// titlebar: the traffic-light buttons stay native and functional, but there's
/// no separate OS-drawn strip above the app's own titlebar row (see
/// `Theme.traffic-light-w` in `app-window.slint`) - left alone, that strip
/// follows the OS light/dark setting rather than this app's own (decoupled)
/// theme toggle. Must run before any Slint window is created - the platform
/// can only be installed once, and Slint installs a default one lazily on
/// first use.
#[cfg(target_os = "macos")]
fn install_macos_titlebar_backend() {
    use winit::platform::macos::WindowAttributesExtMacOS;

    let backend = i_slint_backend_winit::Backend::builder()
        .with_window_attributes_hook(|attrs| {
            attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true)
        })
        .build()
        .expect("failed to build the winit backend");

    slint::platform::set_platform(Box::new(backend))
        .expect("Slint platform must not already be set");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    install_macos_titlebar_backend();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,lito=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Lito v2 - Rust + Slint");

    // Initialize core services
    let agent = Agent::new();
    let memory = MemoryStore::new(None)?;
    let model = memory
        .get_setting("selected_model")?
        .unwrap_or_else(|| "qwen3.5:9b".to_string());
    let dark_mode = memory
        .get_setting("dark_mode")?
        .map(|v| v == "true")
        .unwrap_or(false);
    let ollama_endpoint = memory
        .get_setting("ollama_endpoint")?
        .unwrap_or_else(|| lito_llm::DEFAULT_BASE_URL.to_string());
    let llm = OllamaClient::new(ollama_endpoint.clone(), model);
    let fs_ops = FsOps::new();

    // Command approval policy, restored from persisted settings. Roots are the
    // ones `P1.1` specifies (~/Desktop, ~/Documents, ~/Downloads, ~/Pictures).
    let approval_mode = memory
        .get_setting("approval_mode")?
        .map(|s| ApprovalMode::from_str_lenient(&s))
        .unwrap_or_default();
    let command_allowlist: HashSet<AllowlistKey> = memory
        .get_setting("command_allowlist")?
        .and_then(|s| serde_json::from_str::<Vec<AllowlistKey>>(&s).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default();
    let policy = CommandPolicy::new(
        approval_mode,
        lito_policy::default_roots(),
        command_allowlist,
    );

    // Create channels for UI <-> Core communication
    let (ui_to_core_tx, ui_to_core_rx) = mpsc::unbounded_channel::<UiToCoreMsg>();
    let (core_to_ui_tx, core_to_ui_rx) = mpsc::unbounded_channel::<CoreToUiMsg>();

    // Spawn core agent task
    let agent_handle = tokio::spawn(run_core_agent(
        agent,
        llm,
        memory,
        fs_ops,
        policy,
        ui_to_core_rx,
        core_to_ui_tx,
    ));

    // Create ViewModel and connect channels
    let view_model = ViewModel::new(agent_handle);
    view_model.handle().set_channels(lito_ui::ChannelHandles {
        ui_to_core_tx,
        core_to_ui_rx,
    });

    // Initialize Slint UI
    let window = lito_ui::AppWindow::new(view_model.handle())?;

    // Set up the window reference in the view model
    view_model.handle().set_window(window.as_weak());

    // Push the persisted theme choice now that the window is wired up (a
    // call to `update_ui_state` before `set_window` would be a no-op - see
    // `ViewModelHandle::sync_to_ui`).
    view_model.handle().update_ui_state(|state| {
        state.dark_mode = dark_mode;
        state.ollama_endpoint = ollama_endpoint;
        state.default_ollama_endpoint = lito_llm::DEFAULT_BASE_URL.to_string();
    });

    // Initial ping to check Ollama
    view_model.handle().ping();

    // Run the UI event loop
    window.run()?;

    view_model.shutdown();

    Ok(())
}

/// Truncate a prompt into a short session title (~48 characters, on a char
/// boundary, with an ellipsis if it was cut).
fn derive_title(prompt: &str) -> String {
    const MAX_LEN: usize = 48;
    let trimmed = prompt.trim();
    if trimmed.chars().count() <= MAX_LEN {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(MAX_LEN).collect();
    format!("{}…", truncated.trim_end())
}

/// The sidebar date-group header a session with this `updated_at` belongs
/// under, e.g. "Aujourd'hui", "Hier", or "3 juin" for anything older -
/// compared in UTC calendar days (this is a local single-user app, so we
/// don't track a display timezone separately from the storage timezone).
fn format_group_label(updated_at: i64, now: DateTime<Utc>) -> String {
    let Some(dt) = Utc.timestamp_opt(updated_at, 0).single() else {
        return String::new();
    };
    let days_diff = (now.date_naive() - dt.date_naive()).num_days();
    match days_diff {
        0 => "Aujourd'hui".to_string(),
        1 => "Hier".to_string(),
        _ => format!("{} {}", dt.day(), FRENCH_MONTHS[dt.month0() as usize]),
    }
}

/// The full session list, formatted for the sidebar - sent after startup and
/// after any mutation that could change titles/ordering/subtitles.
fn session_list_msg(memory: &MemoryStore) -> anyhow::Result<CoreToUiMsg> {
    let now = Utc::now();
    let sessions = memory
        .list_sessions()?
        .into_iter()
        .map(|s| SessionListItem {
            id: s.id,
            title: s.title,
            subtitle: s
                .step_count
                .map(|n| format!("{n} étapes"))
                .unwrap_or_default(),
            group_label: format_group_label(s.updated_at, now),
        })
        .collect();
    Ok(CoreToUiMsg::SessionList { sessions })
}

/// Load a session's history + latest plan and build the `SessionSwitched`
/// message for it, along with the `ChatMessage` history the core loop should
/// hold onto while that session is active.
fn session_switched_msg(
    memory: &MemoryStore,
    session_id: &str,
) -> anyhow::Result<(CoreToUiMsg, Vec<ChatMessage>)> {
    let rows = memory.get_recent_messages_for_session(session_id, MAX_HISTORY_MESSAGES)?;
    let history: Vec<ChatMessage> = rows
        .iter()
        .map(|(role, content, _)| ChatMessage {
            role: role.clone(),
            content: content.clone(),
        })
        .collect();
    let plan = memory
        .latest_plan_for_session(session_id)?
        .map(|p| RestoredPlan {
            plan_id: p.plan_id,
            steps: p.steps,
            status: p.status,
            undo_id: p.undo_id,
        });

    Ok((
        CoreToUiMsg::SessionSwitched {
            session_id: session_id.to_string(),
            messages: rows,
            plan,
        },
        history,
    ))
}

/// A command awaiting the user's decision. The core `.await`s
/// [`ApprovalGate::request`]; the UI's `ApprovalResponse` is routed back to
/// the matching `oneshot` by [`drive_with_ui_pump`].
///
/// Cancellation needs no dedicated machinery: dropping the future that's
/// awaiting `request` drops the `oneshot::Receiver`, so `request` resolves to
/// [`ApprovalDecision::Deny`] on its own.
struct ApprovalGate {
    ui_tx: mpsc::UnboundedSender<CoreToUiMsg>,
    pending: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
}

/// The pieces of a `CoreToUiMsg::ApprovalRequest` the caller supplies;
/// `ApprovalGate` mints the `request_id`.
struct ApprovalAsk {
    argv: Vec<String>,
    shell: bool,
    cwd: String,
    resolved_program: String,
    origin: ApprovalOrigin,
    rememberable: bool,
}

impl ApprovalGate {
    fn new(ui_tx: mpsc::UnboundedSender<CoreToUiMsg>) -> Self {
        Self {
            ui_tx,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Send an approval request to the UI and wait for the answer. Resolves to
    /// [`ApprovalDecision::Deny`] if the UI channel is gone or the wait is
    /// dropped (e.g. the plan was cancelled).
    async fn request(&self, ask: ApprovalAsk) -> ApprovalDecision {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(request_id.clone(), tx);

        let sent = self.ui_tx.send(CoreToUiMsg::ApprovalRequest {
            request_id: request_id.clone(),
            argv: ask.argv,
            shell: ask.shell,
            cwd: ask.cwd,
            resolved_program: ask.resolved_program,
            origin: ask.origin,
            rememberable: ask.rememberable,
        });
        if sent.is_err() {
            self.pending.lock().unwrap().remove(&request_id);
            return ApprovalDecision::Deny;
        }

        match rx.await {
            Ok(decision) => decision,
            Err(_) => {
                self.pending.lock().unwrap().remove(&request_id);
                ApprovalDecision::Deny
            }
        }
    }

    /// Deliver the UI's answer to whichever `request` call is waiting on it.
    fn resolve(&self, request_id: &str, decision: ApprovalDecision) {
        if let Some(tx) = self.pending.lock().unwrap().remove(request_id) {
            let _ = tx.send(decision);
        }
    }
}

/// The [`lito_agent::ReconRunner`] the agent uses to run a reconnaissance
/// command mid-planning: judge it, prompt for approval unless it's on the
/// built-in read-only allowlist, run it. Holds only shared refs - it never
/// persists anything (a recon command is transient, so "remember" is a no-op
/// here), so `&CommandPolicy` is enough.
struct GateReconRunner<'a> {
    policy: &'a CommandPolicy,
    gate: &'a ApprovalGate,
}

impl lito_agent::ReconRunner for GateReconRunner<'_> {
    fn run<'a>(&'a self, argv: Vec<String>, cwd: String) -> lito_agent::ReconFuture<'a> {
        Box::pin(async move {
            if argv.is_empty() {
                return Err(lito_agent::ReconRefused("commande vide".to_string()));
            }
            let cwd_path = resolve_cwd((!cwd.is_empty()).then(|| Path::new(&cwd)), self.policy);
            let spec = CommandSpec::new(argv.clone(), false, cwd_path.clone());

            match self.policy.judge(&spec) {
                Verdict::Denied(violation) => {
                    return Err(lito_agent::ReconRefused(violation.to_string()));
                }
                Verdict::AutoAllow => {}
                Verdict::NeedsApproval { rememberable } => {
                    // The built-in read-only allowlist short-circuits the
                    // prompt even in `validation` mode, so `ls` / `git status`
                    // don't freeze generation on a modal.
                    if !lito_policy::readonly::is_readonly(&argv) {
                        let decision = self
                            .gate
                            .request(ApprovalAsk {
                                argv: argv.clone(),
                                shell: false,
                                cwd: cwd_path.display().to_string(),
                                resolved_program: lito_policy::resolved_program_display(&argv[0]),
                                origin: ApprovalOrigin::Recon,
                                rememberable,
                            })
                            .await;
                        if matches!(decision, ApprovalDecision::Deny) {
                            return Err(lito_agent::ReconRefused("commande refusée".to_string()));
                        }
                    }
                }
            }

            match lito_exec::run(&spec).await {
                Ok(outcome) => Ok(summarize_command(&argv, &outcome)),
                Err(e) => Err(lito_agent::ReconRefused(format!(
                    "la commande n'a pas pu s'exécuter : {e}"
                ))),
            }
        })
    }
}

/// Why [`drive_with_ui_pump`] stopped driving a future without it completing.
enum Interrupted {
    /// The user sent `Cancel`.
    Cancelled,
    /// The UI channel closed (window closing).
    ChannelClosed,
}

/// Drive `fut` to completion while still servicing `ui_rx`: `ApprovalResponse`
/// is routed to the waiting `ApprovalGate`, `Cancel` interrupts, and every
/// other message is deferred (its handler runs once `fut` is done). This is
/// the same `biased` `select!` the `SendMessage` handler always used, lifted
/// out so `ExecutePlan` - which now also needs to wait on the UI mid-run -
/// shares it instead of growing a second, subtly different pump.
async fn drive_with_ui_pump<T>(
    fut: impl Future<Output = T>,
    ui_rx: &mut mpsc::UnboundedReceiver<UiToCoreMsg>,
    deferred: &mut VecDeque<UiToCoreMsg>,
    gate: &ApprovalGate,
) -> Result<T, Interrupted> {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            // Biased so a finished future always wins a same-tick race with a
            // late `Cancel` (see the original comment on this select!).
            biased;

            out = &mut fut => return Ok(out),
            incoming = ui_rx.recv() => match incoming {
                Some(UiToCoreMsg::Cancel) => return Err(Interrupted::Cancelled),
                Some(UiToCoreMsg::ApprovalResponse { request_id, decision }) => {
                    gate.resolve(&request_id, decision);
                }
                Some(other) => deferred.push_back(other),
                None => return Err(Interrupted::ChannelClosed),
            },
        }
    }
}

/// The `PolicyState` message for the settings panel: current mode + the
/// remembered-command list, each rendered as "argv  ·  ~/root".
fn policy_state_msg(policy: &CommandPolicy) -> CoreToUiMsg {
    let allowlist = policy
        .allowlist()
        .map(|key| {
            let argv: Vec<String> = serde_json::from_str(&key.canonical).unwrap_or_default();
            AllowlistEntryView {
                key: serde_json::to_string(key).unwrap_or_default(),
                display: format!("{}  ·  {}", argv.join(" "), prettify_home(&key.root)),
            }
        })
        .collect();
    CoreToUiMsg::PolicyState {
        mode: policy.mode().as_str().to_string(),
        allowlist,
    }
}

/// `~`-collapse a path for display.
fn prettify_home(p: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    p.display().to_string()
}

/// Expand a leading `~` against `$HOME`.
fn expand_tilde_path(p: &Path) -> PathBuf {
    if let Ok(rest) = p.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    p.to_path_buf()
}

/// Persist the approval mode + allowlist so they survive a restart.
fn persist_policy(memory: &MemoryStore, policy: &CommandPolicy) -> anyhow::Result<()> {
    memory.set_setting("approval_mode", policy.mode().as_str())?;
    let entries: Vec<&AllowlistKey> = policy.allowlist().collect();
    memory.set_setting("command_allowlist", &serde_json::to_string(&entries)?)?;
    Ok(())
}

/// Core agent loop - handles messages from UI and coordinates services
#[allow(clippy::too_many_arguments)]
async fn run_core_agent(
    agent: Agent,
    mut llm: OllamaClient,
    memory: MemoryStore,
    mut fs_ops: FsOps,
    mut policy: CommandPolicy,
    mut ui_rx: mpsc::UnboundedReceiver<UiToCoreMsg>,
    ui_tx: mpsc::UnboundedSender<CoreToUiMsg>,
) {
    let gate = ApprovalGate::new(ui_tx.clone());
    tracing::info!("Core agent started");

    let sessions = memory.list_sessions().unwrap_or_default();
    let mut active_session_id = if let Some(first) = sessions.first() {
        first.id.clone()
    } else {
        match memory.create_session("Nouvelle tâche") {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("failed to create initial session: {e}");
                String::new()
            }
        }
    };

    let mut history = match session_switched_msg(&memory, &active_session_id) {
        Ok((msg, history)) => {
            let _ = ui_tx.send(msg);
            history
        }
        Err(e) => {
            tracing::error!("failed to load initial session: {e}");
            Vec::new()
        }
    };

    if let Ok(msg) = session_list_msg(&memory) {
        let _ = ui_tx.send(msg);
    }

    let _ = ui_tx.send(policy_state_msg(&policy));

    // Messages that arrived while a generation was in flight (see
    // `handle_send_message`'s call site below) - drained before going back to
    // the channel, so their order relative to each other is preserved.
    let mut deferred: VecDeque<UiToCoreMsg> = VecDeque::new();

    loop {
        let msg = match deferred.pop_front() {
            Some(msg) => msg,
            None => match ui_rx.recv().await {
                Some(msg) => msg,
                None => break,
            },
        };

        let result = match msg {
            UiToCoreMsg::SendMessage { prompt, context } => {
                // Generation is one of two things that run long enough for the
                // user to want them stopped (plan execution is the other), so
                // it's driven alongside the incoming channel rather than
                // blocking it - see `drive_with_ui_pump`. Dropping the future
                // is what actually stops the work: it drops the `stream_chat`
                // receiver, whose sender task then fails to send, returns, and
                // drops the `reqwest` response - closing the HTTP connection
                // to Ollama rather than leaving it draining tokens into
                // nothing.
                let send = handle_send_message(
                    &agent,
                    &llm,
                    &memory,
                    &fs_ops,
                    &policy,
                    &gate,
                    &ui_tx,
                    &active_session_id,
                    &mut history,
                    prompt,
                    context,
                );
                match drive_with_ui_pump(send, &mut ui_rx, &mut deferred, &gate).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => {
                        // Report it as a chat error, then run the connectivity
                        // diagnostic: a generation that failed outright (an
                        // Ollama error, or a request that hung until a client
                        // timeout) is exactly when it's useful, and it will
                        // surface the sign-in modal for a cloud model that
                        // isn't authenticated. `auto = true` keeps it quiet
                        // when the connection actually checks out.
                        tracing::error!("Generation failed: {e}");
                        let _ = ui_tx.send(CoreToUiMsg::Error {
                            code: lito_protocol::ErrorCode::Internal,
                            message: e.to_string(),
                        });
                        let _ = handle_diagnose(&llm, &ui_tx, true).await;
                        Ok(())
                    }
                    Err(_) => {
                        tracing::info!("Generation cancelled");
                        // The user's message was persisted at the top of
                        // `handle_send_message`, so the turn isn't lost on a
                        // restart; only the assistant reply (which never
                        // arrived) and the in-memory `history` push are
                        // skipped - `history` reloads from the DB anyway.
                        let _ = ui_tx.send(CoreToUiMsg::Cancelled);
                        Ok(())
                    }
                }
            }
            UiToCoreMsg::ExecutePlan { plan_id } => {
                let exec = handle_execute_plan(
                    &mut fs_ops,
                    &memory,
                    &mut policy,
                    &gate,
                    &ui_tx,
                    &active_session_id,
                    &mut history,
                    plan_id,
                );
                match drive_with_ui_pump(exec, &mut ui_rx, &mut deferred, &gate).await {
                    Ok(result) => result,
                    // Interrupted mid-plan: whatever ran already ran (and was
                    // journalled). `handle_execute_plan` left the DB status at
                    // "executing"; a future run/undo still sees the undo id.
                    Err(_) => Ok(()),
                }
            }
            UiToCoreMsg::Undo { session_id } => {
                handle_undo(&mut fs_ops, &memory, &ui_tx, session_id).await
            }
            UiToCoreMsg::ApprovalResponse { .. } => {
                // Only meaningful while a plan/generation is being driven by
                // `drive_with_ui_pump`, which handles it there. Here it just
                // lost the race against the work finishing.
                Ok(())
            }
            UiToCoreMsg::SetApprovalMode { mode } => {
                handle_set_approval_mode(&mut policy, &memory, &ui_tx, mode).await
            }
            UiToCoreMsg::ForgetAllowlistEntry { key } => {
                handle_forget_allowlist_entry(&mut policy, &memory, &ui_tx, key).await
            }
            UiToCoreMsg::RejectPlan { plan_id } => {
                handle_reject_plan(&memory, &ui_tx, plan_id).await
            }
            UiToCoreMsg::NewSession => {
                handle_new_session(&memory, &ui_tx, &mut active_session_id, &mut history).await
            }
            UiToCoreMsg::SwitchSession { session_id } => {
                handle_switch_session(
                    &memory,
                    &ui_tx,
                    &mut active_session_id,
                    &mut history,
                    session_id,
                )
                .await
            }
            UiToCoreMsg::SetModel { model } => {
                handle_set_model(&mut llm, &memory, &ui_tx, model).await
            }
            UiToCoreMsg::SetTheme { dark } => handle_set_theme(&memory, dark).await,
            UiToCoreMsg::SetEndpoint { url } => {
                handle_set_endpoint(&mut llm, &memory, &ui_tx, url).await
            }
            // Only meaningful while a generation is in flight, where it's
            // handled by the `select!` above; here it just lost the race
            // against a response that had already finished.
            UiToCoreMsg::Cancel => Ok(()),
            UiToCoreMsg::Ping => handle_ping(&llm, &ui_tx).await,
            UiToCoreMsg::DiagnoseOllama => handle_diagnose(&llm, &ui_tx, false).await,
        };

        if let Err(e) = result {
            tracing::error!("Core agent error: {}", e);
            let _ = ui_tx.send(CoreToUiMsg::Error {
                code: lito_protocol::ErrorCode::Internal,
                message: e.to_string(),
            });
        }
    }

    tracing::info!("Core agent stopped");
}

/// Handle user message - stream a response from the LLM and classify it as
/// either a chat reply or a plan once fully assembled.
#[allow(clippy::too_many_arguments)]
async fn handle_send_message(
    agent: &Agent,
    llm: &OllamaClient,
    memory: &MemoryStore,
    fs_ops: &FsOps,
    policy: &CommandPolicy,
    gate: &ApprovalGate,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    session_id: &str,
    history: &mut Vec<ChatMessage>,
    prompt: String,
    context: String,
) -> anyhow::Result<()> {
    tracing::info!("Responding to: {}", prompt);

    // A session's title is placeholder ("Nouvelle tâche") until its first
    // turn - `history` is only ever empty right after a `NewSession`/
    // `SwitchSession` into a session with no prior messages, so this is a
    // free, exact signal without an extra DB round trip. Computed before the
    // user message is persisted below.
    let is_first_turn = history.is_empty();

    // Persist the user's message up front, *before* the model is involved.
    // Everything past this point can fail, time out, or be cancelled - and
    // `agent.respond` propagates all of that with `?` - so persisting the
    // turn only on success (as this used to) silently dropped the user's
    // message from the conversation on the next app restart whenever the
    // model misbehaved. The assistant reply and any plan are still saved
    // only on success, further down. `history` (the in-memory window sent to
    // the model) is still only appended to on success: `agent.respond` adds
    // `prompt` itself, so pushing it here would duplicate the turn in the
    // model context; a restart reloads `history` from the DB anyway.
    memory.save_message(session_id, "user", &prompt, None)?;
    memory.touch_session(session_id)?;
    if is_first_turn {
        memory.set_session_title(session_id, &derive_title(&prompt))?;
    }
    // Best-effort: a hiccup rebuilding the sidebar must not abort a turn
    // whose user message is already safely stored.
    if let Ok(msg) = session_list_msg(memory) {
        let _ = ui_tx.send(msg);
    }

    if !llm.health_check().await {
        return Err(anyhow::anyhow!("Ollama is not available"));
    }

    let recon = GateReconRunner { policy, gate };

    let mut thinking = String::new();
    let response = agent
        .respond(&prompt, &context, history, llm, fs_ops, &recon, |event| {
            let msg = match event {
                AgentEvent::Token(token) => CoreToUiMsg::Token {
                    content: token.to_string(),
                },
                AgentEvent::Thinking(chunk) => {
                    // Accumulated here as well as forwarded: the UI shows it
                    // live, and the whole trace is persisted with the turn
                    // below so it survives a restart.
                    thinking.push_str(chunk);
                    CoreToUiMsg::Thinking {
                        content: chunk.to_string(),
                    }
                }
                AgentEvent::Querying(description) => CoreToUiMsg::Querying {
                    description: description.to_string(),
                },
                AgentEvent::PlanSteps(steps) => CoreToUiMsg::PlanStreaming {
                    steps: steps.to_vec(),
                },
            };
            let _ = ui_tx.send(msg);
        })
        .await?;

    // Success: record the assistant reply (the user message was already
    // persisted above). Both go into the in-memory window sent to the model
    // and into SQLite so they survive a restart. Plans don't carry raw text,
    // so summarize them into something the model can refer back to later.
    let assistant_text = match &response {
        AgentResponse::Chat(text) => text.clone(),
        AgentResponse::Plan(steps) => {
            let descriptions: Vec<&str> = steps.iter().map(|s| s.description.as_str()).collect();
            format!("Proposed a plan: {}", descriptions.join("; "))
        }
    };

    let thinking = if thinking.is_empty() {
        None
    } else {
        Some(thinking.as_str())
    };
    memory.save_message(session_id, "assistant", &assistant_text, thinking)?;
    memory.touch_session(session_id)?;

    history.push(ChatMessage::user(prompt.clone()));
    history.push(ChatMessage::assistant(assistant_text));
    if history.len() > MAX_HISTORY_MESSAGES {
        let excess = history.len() - MAX_HISTORY_MESSAGES;
        history.drain(0..excess);
    }

    match response {
        AgentResponse::Plan(steps) => {
            let plan_id = PlanId::new().to_string();
            memory.save_plan(session_id, &plan_id, &prompt, &context, &steps)?;
            ui_tx.send(CoreToUiMsg::PlanReady { plan_id, steps })?;
        }
        AgentResponse::Chat(_) => {
            ui_tx.send(CoreToUiMsg::MessageDone)?;
        }
    }

    ui_tx.send(session_list_msg(memory)?)?;

    Ok(())
}

/// What running one step produced.
enum StepDispatch {
    /// A filesystem step, or a command that exited 0.
    Done,
    /// A command that exited 0; `summary` is folded into `[COMMAND RESULTS]`.
    CommandDone { summary: String },
    /// The step could not complete - the plan stops here. `summary` is a
    /// command summary to still record if the command ran but exited non-zero.
    Failed {
        message: String,
        summary: Option<String>,
    },
}

/// Execute a previously-planned and persisted set of steps.
///
/// A `RUN_COMMAND` step is routed to [`dispatch_step`] (policy check ->
/// approval -> `lito_exec`), never to `FsOps`. Any failure - a policy refusal,
/// a denied approval, a non-zero exit, or a filesystem error - stops the plan
/// with a `StepFailed` message and a `"failed"` DB status, instead of the old
/// `?` that left the plan stuck at `"executing"` forever (roadmap P1.4).
#[allow(clippy::too_many_arguments)]
async fn handle_execute_plan(
    fs_ops: &mut FsOps,
    memory: &MemoryStore,
    policy: &mut CommandPolicy,
    gate: &ApprovalGate,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    active_session_id: &str,
    history: &mut Vec<ChatMessage>,
    plan_id: String,
) -> anyhow::Result<()> {
    tracing::info!("Executing plan: {}", plan_id);

    let steps = memory
        .get_plan(&plan_id)?
        .ok_or_else(|| anyhow::anyhow!("no plan found for id {plan_id}"))?;

    // The undo log groups entries by session id, so every step in this run
    // shares one freshly-generated id - unrelated to the chat session this
    // plan belongs to (see `lito_ui::UiState::current_undo_id`'s doc).
    let undo_session = uuid::Uuid::new_v4().to_string();
    memory.update_plan_status(&plan_id, "executing", Some(undo_session.clone()))?;

    let total = steps.len();
    let mut command_results: Vec<String> = Vec::new();

    for (i, step) in steps.iter().enumerate() {
        let step_no = i + 1;
        ui_tx.send(CoreToUiMsg::StepStart {
            step: step_no,
            total,
            operation: step.operation,
        })?;

        let outcome = dispatch_step(
            step,
            step_no,
            total,
            fs_ops,
            memory,
            policy,
            gate,
            ui_tx,
            &undo_session,
        )
        .await;

        match outcome {
            StepDispatch::Done => {
                ui_tx.send(CoreToUiMsg::StepDone {
                    step: step_no,
                    total,
                })?;
            }
            StepDispatch::CommandDone { summary } => {
                command_results.push(summary);
                ui_tx.send(CoreToUiMsg::StepDone {
                    step: step_no,
                    total,
                })?;
            }
            StepDispatch::Failed { message, summary } => {
                if let Some(summary) = summary {
                    command_results.push(summary);
                }
                ui_tx.send(CoreToUiMsg::StepFailed {
                    step: step_no,
                    total,
                    message,
                })?;
                memory.update_plan_status(&plan_id, "failed", Some(undo_session.clone()))?;
                push_command_results(
                    memory,
                    active_session_id,
                    history,
                    &plan_id,
                    &command_results,
                );
                ui_tx.send(session_list_msg(memory)?)?;
                return Ok(());
            }
        }
    }

    ui_tx.send(CoreToUiMsg::PlanDone {
        undo_id: undo_session.clone(),
    })?;
    memory.update_plan_status(&plan_id, "done", Some(undo_session))?;

    push_command_results(
        memory,
        active_session_id,
        history,
        &plan_id,
        &command_results,
    );

    if let Some(owning_session) = memory.get_plan_session_id(&plan_id)? {
        memory.touch_session(&owning_session)?;
    }
    ui_tx.send(session_list_msg(memory)?)?;

    Ok(())
}

/// Run one plan step. Filesystem operations go straight to `FsOps`;
/// `RUN_COMMAND` goes through the policy + approval gate + `lito_exec`. Never
/// returns `Err` for an ordinary step failure - that's a [`StepDispatch::Failed`]
/// so the plan can stop cleanly.
#[allow(clippy::too_many_arguments)]
async fn dispatch_step(
    step: &PlanStep,
    step_no: usize,
    total: usize,
    fs_ops: &mut FsOps,
    memory: &MemoryStore,
    policy: &mut CommandPolicy,
    gate: &ApprovalGate,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    undo_session: &str,
) -> StepDispatch {
    if step.operation != Operation::RunCommand {
        return match fs_ops.execute_step(step, undo_session).await {
            Ok(()) => StepDispatch::Done,
            Err(e) => StepDispatch::Failed {
                message: e.to_string(),
                summary: None,
            },
        };
    }

    let Some(argv) = step.argv.clone().filter(|a| !a.is_empty()) else {
        return StepDispatch::Failed {
            message: "étape RUN_COMMAND sans `argv`".to_string(),
            summary: None,
        };
    };
    let cwd = resolve_cwd(step.cwd.as_deref(), policy);
    let spec = CommandSpec::new(argv.clone(), step.shell, cwd.clone());

    match policy.judge(&spec) {
        Verdict::Denied(violation) => {
            return StepDispatch::Failed {
                message: format!("{} — {}", ErrorCode::PermissionDenied, violation),
                summary: None,
            };
        }
        Verdict::AutoAllow => {}
        Verdict::NeedsApproval { rememberable } => {
            let decision = gate
                .request(ApprovalAsk {
                    argv: argv.clone(),
                    shell: step.shell,
                    cwd: cwd.display().to_string(),
                    resolved_program: lito_policy::resolved_program_display(&argv[0]),
                    origin: ApprovalOrigin::PlanStep {
                        step: step_no,
                        total,
                    },
                    rememberable,
                })
                .await;
            match decision {
                ApprovalDecision::Deny => {
                    return StepDispatch::Failed {
                        message: "commande refusée".to_string(),
                        summary: None,
                    };
                }
                ApprovalDecision::Allow => {}
                ApprovalDecision::AllowAndRemember => {
                    if let Some(key) = policy.allowlist_key(&spec) {
                        policy.insert_allow(key);
                        if let Err(e) = persist_policy(memory, policy) {
                            tracing::warn!("failed to persist allowlist: {e}");
                        }
                        let _ = ui_tx.send(policy_state_msg(policy));
                    }
                }
            }
        }
    }

    let outcome = match lito_exec::run(&spec).await {
        Ok(outcome) => outcome,
        Err(e) => {
            return StepDispatch::Failed {
                message: format!("la commande n'a pas pu s'exécuter : {e}"),
                summary: None,
            };
        }
    };

    let _ = ui_tx.send(CoreToUiMsg::CommandOutput {
        step: step_no,
        exit_code: outcome.exit_code,
        stdout: outcome.stdout.clone(),
        stderr: outcome.stderr.clone(),
        truncated: outcome.truncated,
    });

    // Journal the (irreversible) command in SQLite's undo_log - `log_undo`
    // exists for exactly this and had no caller before now. `handle_undo`
    // reads it back to count what it couldn't reverse.
    let action = serde_json::json!({
        "type": "command",
        "argv": argv,
        "cwd": cwd.display().to_string(),
        "shell": step.shell,
        "exit_code": outcome.exit_code,
    })
    .to_string();
    if let Err(e) = memory.log_undo(undo_session, &action) {
        tracing::warn!("failed to journal command: {e}");
    }

    let summary = summarize_command(&argv, &outcome);
    if outcome.exit_code == Some(0) {
        StepDispatch::CommandDone { summary }
    } else {
        StepDispatch::Failed {
            message: format!(
                "la commande s'est terminée avec le code {}",
                outcome
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "inconnu".to_string())
            ),
            summary: Some(summary),
        }
    }
}

/// Resolve a `RUN_COMMAND` step's `cwd`: `~`-expand and canonicalize what the
/// model gave, or fall back to the first authorized root when it gave nothing.
fn resolve_cwd(cwd: Option<&Path>, policy: &CommandPolicy) -> PathBuf {
    let raw = match cwd {
        Some(p) => expand_tilde_path(p),
        None => policy
            .roots()
            .first()
            .cloned()
            .unwrap_or_else(std::env::temp_dir),
    };
    raw.canonicalize().unwrap_or(raw)
}

/// A compact, capped rendering of a command's result for the `[COMMAND RESULTS]`
/// message folded back into the conversation.
fn summarize_command(argv: &[String], outcome: &CommandOutcome) -> String {
    const BODY_CAP: usize = 800;
    let mut body = outcome.stdout.trim_end().to_string();
    let stderr = outcome.stderr.trim_end();
    if !stderr.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str("[stderr] ");
        body.push_str(stderr);
    }
    format!(
        "$ {}\n(exit {}){}\n{}",
        argv.join(" "),
        outcome
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".to_string()),
        if outcome.truncated {
            " [sortie tronquée]"
        } else {
            ""
        },
        truncate_chars(&body, BODY_CAP)
    )
}

/// Truncate to at most `max` chars on a char boundary, adding an ellipsis if cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

/// Fold the run's command outputs into the active conversation's in-memory
/// window (never SQLite, never another session) so the next turn has them.
/// No automatic re-prompt of the model.
fn push_command_results(
    memory: &MemoryStore,
    active_session_id: &str,
    history: &mut Vec<ChatMessage>,
    plan_id: &str,
    results: &[String],
) {
    if results.is_empty() {
        return;
    }
    let owning = memory.get_plan_session_id(plan_id).ok().flatten();
    if owning.as_deref() != Some(active_session_id) {
        return;
    }
    let joined = results.join("\n\n");
    history.push(ChatMessage::user(format!(
        "[COMMAND RESULTS]\n{}",
        truncate_chars(&joined, 4000)
    )));
    if history.len() > MAX_HISTORY_MESSAGES {
        let excess = history.len() - MAX_HISTORY_MESSAGES;
        history.drain(0..excess);
    }
}

/// Handle undo request. `FsOps::undo` reverses the filesystem operations it
/// logged; commands were never in its log (they have no inverse), so we read
/// SQLite's `undo_log` to count how many of them ran and report that.
async fn handle_undo(
    fs_ops: &mut FsOps,
    memory: &MemoryStore,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    session_id: String,
) -> anyhow::Result<()> {
    tracing::info!("Undoing session: {}", session_id);

    fs_ops.undo(&session_id).await?;

    let irreversible_commands = memory
        .get_undo_actions(&session_id)
        .unwrap_or_default()
        .iter()
        .filter(|action| {
            serde_json::from_str::<serde_json::Value>(action)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str().map(str::to_string)))
                .as_deref()
                == Some("command")
        })
        .count();

    ui_tx.send(CoreToUiMsg::UndoDone {
        irreversible_commands,
    })?;

    Ok(())
}

/// Switch the command approval mode, persist it, and echo the new state.
async fn handle_set_approval_mode(
    policy: &mut CommandPolicy,
    memory: &MemoryStore,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    mode: String,
) -> anyhow::Result<()> {
    tracing::info!("Approval mode -> {mode}");
    policy.set_mode(ApprovalMode::from_str_lenient(&mode));
    persist_policy(memory, policy)?;
    ui_tx.send(policy_state_msg(policy))?;
    Ok(())
}

/// Revoke one remembered command, persist, and echo the new state.
async fn handle_forget_allowlist_entry(
    policy: &mut CommandPolicy,
    memory: &MemoryStore,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    key: String,
) -> anyhow::Result<()> {
    if let Ok(parsed) = serde_json::from_str::<AllowlistKey>(&key) {
        if policy.remove_allow(&parsed) {
            persist_policy(memory, policy)?;
        }
    }
    ui_tx.send(policy_state_msg(policy))?;
    Ok(())
}

/// Reject a proposed plan without executing it - nothing touched the
/// filesystem yet, so this is just a status update, no undo involved.
async fn handle_reject_plan(
    memory: &MemoryStore,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    plan_id: String,
) -> anyhow::Result<()> {
    tracing::info!("Rejecting plan: {}", plan_id);

    memory.update_plan_status(&plan_id, "rejected", None)?;
    ui_tx.send(CoreToUiMsg::PlanRejected { plan_id })?;

    Ok(())
}

/// Start a brand new conversation and switch into it.
async fn handle_new_session(
    memory: &MemoryStore,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    active_session_id: &mut String,
    history: &mut Vec<ChatMessage>,
) -> anyhow::Result<()> {
    let session_id = memory.create_session("Nouvelle tâche")?;
    *active_session_id = session_id.clone();
    history.clear();

    ui_tx.send(CoreToUiMsg::SessionSwitched {
        session_id,
        messages: Vec::new(),
        plan: None,
    })?;
    ui_tx.send(session_list_msg(memory)?)?;

    Ok(())
}

/// Switch the active conversation, reloading its history and restoring
/// whatever plan-panel state it last had.
async fn handle_switch_session(
    memory: &MemoryStore,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    active_session_id: &mut String,
    history: &mut Vec<ChatMessage>,
    session_id: String,
) -> anyhow::Result<()> {
    let (msg, new_history) = session_switched_msg(memory, &session_id)?;
    *active_session_id = session_id;
    *history = new_history;
    ui_tx.send(msg)?;

    Ok(())
}

/// Handle ping - check Ollama connectivity and list available models
async fn handle_ping(
    llm: &OllamaClient,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
) -> anyhow::Result<()> {
    let ollama_ok = llm.health_check().await;
    let available_models = llm.list_models().await.unwrap_or_default();
    ui_tx.send(CoreToUiMsg::Pong {
        ollama: ollama_ok,
        model: llm.model().to_string(),
        available_models,
        endpoint: llm.base_url().to_string(),
    })?;
    Ok(())
}

/// Run the Ollama connectivity diagnostic (`OllamaClient::diagnose`) and
/// translate the outcome into a `DiagnosisResult` the UI shows in a modal.
///
/// `auto` is set when this runs on its own after a failed generation rather
/// than from the settings-panel button: in that case a clean result is
/// swallowed (the user didn't ask, and a "connection OK" popup right after an
/// error is just noise), while any real problem - and especially a cloud
/// model that needs `ollama signin` - still surfaces.
async fn handle_diagnose(
    llm: &OllamaClient,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    auto: bool,
) -> anyhow::Result<()> {
    let diagnosis = llm.diagnose().await;
    tracing::info!(
        "Ollama diagnosis ({}): {diagnosis:?}",
        if auto { "auto" } else { "manual" }
    );

    let (ok, needs_signin, title, detail) = describe_diagnosis(diagnosis, llm.model());
    if auto && ok {
        return Ok(());
    }
    ui_tx.send(CoreToUiMsg::DiagnosisResult {
        ok,
        needs_signin,
        title,
        detail,
    })?;
    Ok(())
}

/// Map an `OllamaDiagnosis` to the `(ok, needs_signin, title, detail)` the UI
/// renders. All copy is French, matching the rest of the app's chrome.
fn describe_diagnosis(diagnosis: OllamaDiagnosis, model: &str) -> (bool, bool, String, String) {
    match diagnosis {
        OllamaDiagnosis::Ok => (
            true,
            false,
            "Connexion opérationnelle".to_string(),
            format!("Le modèle « {model} » répond normalement."),
        ),
        OllamaDiagnosis::DaemonUnreachable { endpoint, detail } => (
            false,
            false,
            "Ollama injoignable".to_string(),
            format!(
                "Impossible de contacter Ollama à l'adresse {endpoint}. Vérifie qu'Ollama est \
                 lancé (« ollama serve ») et que l'endpoint est correct dans les réglages.\n\n\
                 Détail : {detail}"
            ),
        ),
        OllamaDiagnosis::ModelNotFound { model } => (
            false,
            false,
            "Modèle non installé".to_string(),
            format!(
                "Le modèle « {model} » n'est pas présent localement. Récupère-le avec \
                 « ollama pull {model} », ou choisis-en un autre dans le sélecteur en haut."
            ),
        ),
        OllamaDiagnosis::CloudSignInRequired { model } => (
            false,
            true,
            "Connexion à Ollama requise".to_string(),
            format!(
                "« {model} » est un modèle Ollama Cloud, et le serveur a répondu par une erreur \
                 d'authentification : il faut être connecté à un compte Ollama."
            ),
        ),
        OllamaDiagnosis::CloudUnresponsive { model, waited_secs } => (
            false,
            true,
            "Le modèle cloud n'a pas répondu".to_string(),
            format!(
                "« {model} » est un modèle Ollama Cloud et n'a rien renvoyé en {waited_secs} s. \
                 Rien ne tourne en local pour un modèle cloud, donc pas de chargement de poids ici \
                 — mais le premier appel peut quand même traîner :\n\n\
                 • le service hébergé planifie un gros modèle, ou la connexion au service s'établit \
                 — ça peut prendre jusqu'à une ou deux minutes au premier appel. Réessaie.\n\
                 • une session non authentifiée — si le problème persiste, connecte-toi avec \
                 « ollama signin ».\n\n\
                 En attendant, un modèle local dans le sélecteur en haut répond immédiatement."
            ),
        ),
        OllamaDiagnosis::ServerError { status, body } => (
            false,
            false,
            format!("Erreur Ollama ({status})"),
            if body.trim().is_empty() {
                format!("Ollama a renvoyé le statut HTTP {status}.")
            } else {
                format!("Ollama a renvoyé le statut HTTP {status} :\n\n{body}")
            },
        ),
        OllamaDiagnosis::ModelUnresponsive { model, waited_secs } => (
            false,
            false,
            "Le modèle ne répond pas".to_string(),
            format!(
                "« {model} » n'a produit aucun jeton en {waited_secs} s. Il est peut-être en \
                 cours de chargement (réessaie), trop volumineux pour cette machine, ou bloqué."
            ),
        ),
    }
}

/// Handle an endpoint switch requested from the settings panel: point the
/// live client at the new URL, persist the choice, then report fresh
/// connectivity/model state exactly like a ping would - the same pattern as
/// `handle_set_model`, so a bad URL shows up as "hors ligne" instead of
/// being accepted silently.
async fn handle_set_endpoint(
    llm: &mut OllamaClient,
    memory: &MemoryStore,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    url: String,
) -> anyhow::Result<()> {
    tracing::info!("Switching Ollama endpoint to: {}", url);

    llm.set_base_url(url.clone());
    memory.set_setting("ollama_endpoint", &url)?;

    handle_ping(llm, ui_tx).await
}

/// Handle a model switch requested from the UI: update the live client,
/// persist the choice, then report fresh connectivity/model state exactly
/// like a ping would.
async fn handle_set_model(
    llm: &mut OllamaClient,
    memory: &MemoryStore,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    model: String,
) -> anyhow::Result<()> {
    tracing::info!("Switching model to: {}", model);

    llm.set_model(model.clone());
    memory.set_setting("selected_model", &model)?;

    handle_ping(llm, ui_tx).await
}

/// Persist a theme choice. The UI already applied it optimistically (see
/// `ViewModelHandle::toggle_theme`), so this is fire-and-forget - no reply
/// needed.
async fn handle_set_theme(memory: &MemoryStore, dark: bool) -> anyhow::Result<()> {
    tracing::info!("Setting theme: dark={dark}");
    memory.set_setting("dark_mode", if dark { "true" } else { "false" })?;
    Ok(())
}
