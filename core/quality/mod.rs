pub mod overlay_quality;
pub mod qube_daemon;
pub mod qube_report;
pub mod teacher;

pub use teacher::{
    MergeMode, MergedDelivery, TeacherInput, TeacherReport, merge_live_whisper, report_to_html,
    teach,
};
