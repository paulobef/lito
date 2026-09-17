//! AppWindow wrapper: wires the Slint-generated `MainWindow` component up to
//! a [`ViewModelHandle`].

use slint::ComponentHandle;

use crate::generated::MainWindow;
use crate::view_model::ViewModelHandle;

/// Wrapper around the Slint-generated `MainWindow` component
pub struct AppWindow {
    window: MainWindow,
    view_model: ViewModelHandle,
}

impl AppWindow {
    /// Create a new AppWindow with the given ViewModel
    pub fn new(view_model: ViewModelHandle) -> Result<Self, slint::PlatformError> {
        let window = MainWindow::new()?;
        let vm = view_model.clone();

        window.on_send_message(move |prompt, context| {
            vm.send_message(prompt.to_string(), context.to_string());
        });

        let vm = view_model.clone();
        window.on_execute_plan(move |plan_id| {
            vm.execute_plan(plan_id.to_string());
        });

        let vm = view_model.clone();
        window.on_undo(move |session_id| {
            vm.undo(session_id.to_string());
        });

        let vm = view_model.clone();
        window.on_reject_plan(move |plan_id| {
            vm.reject_plan(plan_id.to_string());
        });

        let vm = view_model.clone();
        window.on_open_plan(move || {
            vm.open_plan();
        });

        let vm = view_model.clone();
        window.on_close_plan(move || {
            vm.close_plan();
        });

        let vm = view_model.clone();
        window.on_new_session(move || {
            vm.new_session();
        });

        let vm = view_model.clone();
        window.on_select_session(move |session_id| {
            vm.select_session(session_id.to_string());
        });

        let vm = view_model.clone();
        window.on_search_sessions(move |text| {
            vm.search_sessions(text.to_string());
        });

        let vm = view_model.clone();
        window.on_select_model(move |model| {
            vm.set_model(model.to_string());
        });

        let vm = view_model.clone();
        window.on_toggle_theme(move |dark| {
            vm.toggle_theme(dark);
        });

        let vm = view_model.clone();
        window.on_set_endpoint(move |url| {
            vm.set_endpoint(url.to_string());
        });

        let vm = view_model.clone();
        window.on_ping(move || {
            vm.ping();
        });

        let vm = view_model.clone();
        window.on_cancel(move || {
            vm.cancel();
        });

        let vm = view_model.clone();
        window.on_copy_message(move |text| {
            vm.copy_message(text.to_string());
        });

        let vm = view_model.clone();
        window.on_approve_command(move |request_id, decision| {
            vm.respond_to_approval(request_id.to_string(), decision.to_string());
        });

        let vm = view_model.clone();
        window.on_set_approval_mode(move |mode| {
            vm.set_approval_mode(mode.to_string());
        });

        let vm = view_model.clone();
        window.on_forget_allowlist_entry(move |key| {
            vm.forget_allowlist_entry(key.to_string());
        });

        let vm = view_model.clone();
        window.on_diagnose_ollama(move || {
            vm.diagnose_ollama();
        });

        let vm = view_model.clone();
        window.on_dismiss_diagnosis(move || {
            vm.dismiss_diagnosis();
        });

        // Initial UI state
        view_model.update_ui_state(|ui| {
            ui.is_processing = false;
            ui.ollama_connected = false;
        });

        Ok(Self { window, view_model })
    }

    /// Show the window and run the event loop
    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }

    /// Get a weak reference to the window for external updates
    pub fn as_weak(&self) -> slint::Weak<MainWindow> {
        self.window.as_weak()
    }

    /// Get the ViewModel handle
    pub fn view_model(&self) -> ViewModelHandle {
        self.view_model.clone()
    }
}
