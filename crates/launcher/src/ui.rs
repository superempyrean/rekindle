//! What the setup window shows and does, without drawing anything:
//! [`Model`] holds the page, [`Model::act`] takes what the user clicked and
//! returns the [`Job`] to run, and [`Model::finished`] takes the job's
//! result. `window.rs` draws it with egui; the tests drive it directly.
//!
//! The text is for someone who has never used a terminal: one big button
//! per page, no paths unless they open "Details", and the client called by
//! its name.

use std::path::PathBuf;

/// What the client is called in what the user reads.
pub const CLIENT: &str = "Discord";

/// The page the window shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Page {
    /// Not installed yet.
    Welcome,
    /// Installed already (the setup was opened again).
    Installed {
        installed: String,
    },
    /// A job is running.
    Working(Job),
    /// Installed: offer to open the client.
    InstallDone,
    /// Installed, but no version came up with the client, so it runs
    /// without rekindle. Offer to check again.
    InstalledPlain {
        details: String,
    },
    Uninstall {
        delete_plugins: bool,
    },
    Uninstalled,
    /// Something failed. `details` is for whoever helps the user.
    Problem {
        text: String,
        details: String,
    },
    /// The window can close.
    Closed,
}

/// Work a button starts, run off the window's thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Install,
    Launch,
    Uninstall { delete_plugins: bool },
}

impl Job {
    /// What the window says while it runs.
    pub fn label(&self) -> String {
        match self {
            Job::Install => "Installing rekindle…".into(),
            Job::Launch => format!("Starting {CLIENT}…"),
            Job::Uninstall { .. } => "Removing rekindle…".into(),
        }
    }
}

/// What the user did.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// The main button of the page.
    Primary,
    /// The client program the user picked (`None`: they cancelled).
    Picked(Option<PathBuf>),
    Open,
    StartUninstall,
    ToggleDeletePlugins,
    Cancel,
    Close,
}

/// How a job ended.
#[derive(Debug)]
pub enum Outcome {
    Done,
    /// The client started without rekindle, and why.
    Plain(String),
    Failed(anyhow::Error),
}

/// The window's state.
#[derive(Debug)]
pub struct Model {
    pub page: Page,
    /// The version the setup carries (`None` when it's the installed copy).
    pub version: Option<String>,
    /// The client program, once known.
    pub client: Option<PathBuf>,
    /// Ask the window to show the "Where is Discord?" file picker.
    pub want_picker: bool,
}

impl Model {
    /// The setup window: `installed` is the version already installed, if any.
    pub fn setup(version: &str, installed: Option<String>, client: Option<PathBuf>) -> Self {
        let page = match installed {
            Some(installed) => Page::Installed { installed },
            None => Page::Welcome,
        };
        Self {
            version: Some(version.to_string()),
            client,
            ..Self::showing(page)
        }
    }

    /// The uninstall window (what Settings › Apps runs).
    pub fn uninstall() -> Self {
        Self::showing(Page::Uninstall {
            delete_plugins: false,
        })
    }

    /// A window that says what failed.
    pub fn problem(e: &anyhow::Error) -> Self {
        Self::showing(problem(e))
    }

    /// `page`, with no version or client known.
    fn showing(page: Page) -> Self {
        Self {
            page,
            version: None,
            client: None,
            want_picker: false,
        }
    }

    /// The label of the page's main button.
    pub fn primary_label(&self) -> Option<String> {
        let v = self.version.as_deref().unwrap_or("");
        Some(match &self.page {
            Page::Welcome if self.client.is_none() => format!("Find {CLIENT}…"),
            Page::Welcome => "Install".into(),
            Page::Installed { installed } => match semver_cmp(v, installed) {
                std::cmp::Ordering::Greater => format!("Update to {v}"),
                std::cmp::Ordering::Equal => "Repair".into(),
                std::cmp::Ordering::Less => format!("Install {v} instead"),
            },
            Page::InstallDone => format!("Open {CLIENT}"),
            Page::InstalledPlain { .. } => "Try again".into(),
            Page::Uninstall { .. } => "Uninstall".into(),
            Page::Uninstalled | Page::Problem { .. } => "Close".into(),
            Page::Working(_) | Page::Closed => return None,
        })
    }

    /// Apply what the user did; returns the job to run, if any.
    pub fn act(&mut self, action: Action) -> Option<Job> {
        match (&self.page, action) {
            (Page::Working(_), _) => None,
            (_, Action::Close)
            | (Page::Uninstall { .. }, Action::Cancel)
            | (Page::Uninstalled | Page::Problem { .. }, Action::Primary) => {
                self.page = Page::Closed;
                None
            }
            (Page::Welcome, Action::Primary) if self.client.is_none() => {
                self.want_picker = true;
                None
            }
            (_, Action::Picked(picked)) => {
                self.want_picker = false;
                if picked.is_some() {
                    self.client = picked;
                }
                None
            }
            (
                Page::Welcome | Page::Installed { .. } | Page::InstalledPlain { .. },
                Action::Primary,
            ) => Some(self.start(Job::Install)),
            (Page::Installed { .. } | Page::InstallDone, Action::Open)
            | (Page::InstallDone, Action::Primary) => Some(self.start(Job::Launch)),
            (Page::Installed { .. }, Action::StartUninstall) => {
                self.page = Page::Uninstall {
                    delete_plugins: false,
                };
                None
            }
            (Page::Uninstall { delete_plugins }, Action::ToggleDeletePlugins) => {
                self.page = Page::Uninstall {
                    delete_plugins: !delete_plugins,
                };
                None
            }
            (Page::Uninstall { delete_plugins }, Action::Primary) => {
                let delete_plugins = *delete_plugins;
                Some(self.start(Job::Uninstall { delete_plugins }))
            }
            _ => None,
        }
    }

    /// Show `job` as running, and return it to run.
    fn start(&mut self, job: Job) -> Job {
        self.page = Page::Working(job.clone());
        job
    }

    /// A job finished.
    pub fn finished(&mut self, job: &Job, outcome: Outcome) {
        self.page = match (job, outcome) {
            (_, Outcome::Failed(e)) => problem(&e),
            (Job::Install, Outcome::Plain(details)) => Page::InstalledPlain { details },
            (Job::Install, Outcome::Done) => Page::InstallDone,
            // Opening the client is the whole job; placing rekindle beside
            // it, and saying when that failed, happened at install.
            (Job::Launch, _) => Page::Closed,
            (Job::Uninstall { .. }, _) => Page::Uninstalled,
        };
    }
}

fn problem(e: &anyhow::Error) -> Page {
    Page::Problem {
        text: e.to_string(),
        details: format!("{e:#}"),
    }
}

/// `a` against `b` as versions (unparseable ones compare as text).
fn semver_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(m: &mut Model, action: Action, outcome: impl FnOnce() -> Outcome) {
        if let Some(job) = m.act(action) {
            m.finished(&job, outcome());
        }
    }

    #[test]
    fn a_first_install_asks_for_the_client_then_installs_and_opens_it() {
        let mut m = Model::setup("0.2.0", None, None);
        assert_eq!(m.primary_label().as_deref(), Some("Find Discord…"));
        assert_eq!(m.act(Action::Primary), None);
        assert!(m.want_picker);
        m.act(Action::Picked(None));
        assert_eq!(
            m.page,
            Page::Welcome,
            "cancelling the picker changes nothing"
        );
        m.act(Action::Picked(Some("C:/Discord/Discord.exe".into())));
        assert_eq!(m.primary_label().as_deref(), Some("Install"));
        assert_eq!(m.act(Action::Primary), Some(Job::Install));
        assert_eq!(m.page, Page::Working(Job::Install));
        assert_eq!(
            m.act(Action::Close),
            None,
            "nothing happens while a job runs"
        );
        m.finished(&Job::Install, Outcome::Done);
        assert_eq!(m.page, Page::InstallDone);
        run(&mut m, Action::Primary, || Outcome::Done);
        assert_eq!(m.page, Page::Closed);
    }

    #[test]
    fn opening_the_setup_again_offers_update_repair_or_uninstall() {
        assert_eq!(
            Model::setup("0.3.0", Some("0.2.0".into()), None)
                .primary_label()
                .as_deref(),
            Some("Update to 0.3.0")
        );
        assert_eq!(
            Model::setup("0.2.0", Some("0.2.0".into()), None)
                .primary_label()
                .as_deref(),
            Some("Repair")
        );
        assert_eq!(
            Model::setup("0.10.0", Some("0.9.0".into()), None)
                .primary_label()
                .as_deref(),
            Some("Update to 0.10.0")
        );
        let mut m = Model::setup("0.2.0", Some("0.2.0".into()), Some("C:/D.exe".into()));
        m.act(Action::StartUninstall);
        assert_eq!(
            m.page,
            Page::Uninstall {
                delete_plugins: false
            }
        );
    }

    #[test]
    fn uninstall_keeps_plugins_unless_ticked() {
        let mut m = Model::uninstall();
        m.act(Action::ToggleDeletePlugins);
        assert_eq!(
            m.act(Action::Primary),
            Some(Job::Uninstall {
                delete_plugins: true
            })
        );
        m.finished(
            &Job::Uninstall {
                delete_plugins: true,
            },
            Outcome::Done,
        );
        assert_eq!(m.page, Page::Uninstalled);
        let mut m = Model::uninstall();
        assert_eq!(
            m.act(Action::Primary),
            Some(Job::Uninstall {
                delete_plugins: false
            })
        );
        let mut m = Model::uninstall();
        m.act(Action::Cancel);
        assert_eq!(m.page, Page::Closed);
    }

    #[test]
    fn a_failure_says_what_happened_and_opening_the_client_closes_the_window() {
        let mut m = Model::setup("0.2.0", None, Some("C:/D.exe".into()));
        run(&mut m, Action::Primary, || {
            Outcome::Failed(anyhow::anyhow!("disk full").context("writing state.json"))
        });
        let Page::Problem { text, details } = &m.page else {
            panic!("{:?}", m.page)
        };
        assert_eq!(text, "writing state.json");
        assert!(details.contains("disk full"));
        let mut m = Model::setup("0.2.0", None, Some("C:/D.exe".into()));
        run(&mut m, Action::Primary, || Outcome::Done);
        run(&mut m, Action::Open, || Outcome::Done);
        assert_eq!(m.page, Page::Closed);
    }

    #[test]
    fn an_install_that_didnt_come_up_says_so_and_can_check_again() {
        let mut m = Model::setup("0.2.0", Some("0.1.0".into()), Some("C:/D.exe".into()));
        run(&mut m, Action::Primary, || {
            Outcome::Plain("the client ended (exit code 0) before rekindle reported".into())
        });
        let Page::InstalledPlain { details } = &m.page else {
            panic!("{:?}", m.page)
        };
        assert!(details.contains("exit code 0"));
        assert_eq!(m.primary_label().as_deref(), Some("Try again"));
        assert_eq!(m.act(Action::Primary), Some(Job::Install));
        m.finished(&Job::Install, Outcome::Done);
        assert_eq!(m.page, Page::InstallDone);
    }
}
