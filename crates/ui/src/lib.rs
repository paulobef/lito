//! Lito UI - Slint-based UI components and ViewModels

mod app_window;
mod highlight;
mod markdown;
mod view_model;

/// Slint-generated bindings, kept in their own module since the generated
/// component name (`MainWindow`) would otherwise collide with our
/// `AppWindow` wrapper below.
mod generated {
    slint::include_modules!();
}

pub use app_window::AppWindow;
pub use generated::{
    AllowlistEntryData, ChatMessageData, CodeLineData, CodeSpanData, MainWindow, MessageBlockData,
    PlanStepData, SessionRowData,
};
pub use view_model::{
    AllowlistEntryView, ApprovalDecision, ApprovalOrigin, ChannelHandles, CoreToUiMsg,
    RestoredPlan, SessionListItem, UiToCoreMsg, ViewModel, ViewModelHandle,
};
