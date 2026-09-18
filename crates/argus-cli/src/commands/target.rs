//! Selection of the application to observe.

use argus_core::accessibility::AppTarget;

/// Which application to read. Defaults to the frontmost application.
#[derive(Debug, clap::Args)]
pub(crate) struct AppTargetArgs {
    /// Application name or bundle identifier (e.g. `Calculator` or
    /// `com.apple.calculator`). Works for background applications.
    #[arg(long, value_name = "NAME", conflicts_with = "pid")]
    app: Option<String>,

    /// Process identifier of the application.
    #[arg(long, value_name = "PID")]
    pid: Option<u32>,
}

impl AppTargetArgs {
    pub(crate) fn target(&self) -> AppTarget {
        match (&self.app, self.pid) {
            (Some(name), _) => AppTarget::Name(name.clone()),
            (None, Some(pid)) => AppTarget::Pid(pid),
            (None, None) => AppTarget::Frontmost,
        }
    }
}
