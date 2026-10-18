mod tracker;

use super::profile::{AgentProfile, NativeSupport, Resume};
use crate::stream::TitleFilter;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "omp",
    display_name: "OMP",
    commands: &["omp", "oh-my-pi"],
    // omp reports state only: no `title`, no turn summary, and the working indicator lives in
    // its OSC 0 title spinner.
    native: NativeSupport::Partial,
    rules: include_str!("rules.toml"),
    titles: TitleFilter::Omp,
    resume: Resume::flags(&["--continue", "-c", "--resume", "-r", "--session"]),
    tracker: Some(|| Box::new(tracker::OmpTracker::default())),
    summary: "Native state only; w1 adds working/title/summary/progress (session JSONL) and strips the title spinner",
    ..AgentProfile::DEFAULTS
};
