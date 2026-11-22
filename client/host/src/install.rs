//! The install the shim loaded this version from: whether it came up,
//! updates from the release channel, and which session this is.

use rekindle_install::env;

/// A real launch (the launcher's), not a test: rekindle loads the user's
/// installed plugins with their saved settings and network, runs no checks
/// against them, and stays attached.
pub(crate) fn installed_mode() -> bool {
    std::env::var_os(env::INSTALLED).is_some_and(|v| v == "1")
}

/// `REKINDLE_HOST_STORE`: the self-test's store run. rekindle uses the home
/// in `REKINDLE_HOME` with the network on, as a real launch does, and the
/// drive installs a plugin from the community index through rekindle's
/// panel. Off unless `REKINDLE_HOME` is set, so a test never installs into
/// the user's own home.
pub(crate) fn store_mode() -> bool {
    std::env::var_os("REKINDLE_HOST_STORE").is_some_and(|v| v == "1")
        && std::env::var_os("REKINDLE_HOME").is_some()
}

/// What the store run left installed: `<id>:<trust>:<state>` for each
/// plugin, comma-separated, or `none`.
pub(crate) fn store_installed() -> String {
    let Ok(home) = rekindle_plugin_host::home::Home::from_env() else {
        return "none".into();
    };
    let list: Vec<String> = home
        .installed()
        .unwrap_or_default()
        .iter()
        .map(|i| {
            format!(
                "{}:{}:{}",
                i.id,
                i.trust
                    .as_ref()
                    .map_or_else(|| "?".into(), |t| format!("{:?}", t.trust)),
                if i.is_ready() { "on" } else { "off" }
            )
        })
        .collect();
    if list.is_empty() {
        "none".into()
    } else {
        list.join(",")
    }
}

/// Record in the install's `state.json` whether version `v` came up, so the
/// next launch keeps it or falls back.
pub(crate) fn report_to_install(v: &str, report: &str) {
    let Ok(root) = rekindle_install::Install::default_root() else {
        return;
    };
    let install = rekindle_install::Install::new(root);
    let Ok(mut state) = install.state() else {
        return;
    };
    let up = rekindle_install::healthy(report);
    state.reported(v, up.is_ok());
    if let Err(e) = install.save(&state) {
        install.log(&format!("{v}: saving state.json failed: {e:#}"));
    }
    match up {
        Ok(()) => install.log(&format!("{v} came up in the client")),
        Err(e) => install.log(&format!("{v} didn't come up in the client: {e:#}")),
    }
}

/// Check the release channel for a newer rekindle and launcher, now and
/// every six hours, on a thread of its own. What it downloads is the
/// version the next launch loads (`rekindle_release::update`).
pub(crate) fn start_updates() {
    let spawned = std::thread::Builder::new()
        .name("rekindle-self-update".into())
        .spawn(|| {
            loop {
                let did = update_install();
                if let Ok(path) = std::env::var(env::UPDATED) {
                    // Only a check reads it, and a check that never gets it
                    // fails on its own.
                    let _ = std::fs::write(path, &did);
                }
                std::thread::sleep(rekindle_plugin_host::updates::CHECK_INTERVAL);
            }
        });
    // Without the thread, this launch doesn't update; the next one tries
    // again.
    drop(spawned);
}

/// One update check of the install; what it did, for the log and a test.
fn update_install() -> String {
    let install = match rekindle_install::Install::default_root() {
        Ok(root) => rekindle_install::Install::new(root),
        Err(e) => return format!("error: {e:#}"),
    };
    let location = rekindle_plugin_host::home::Home::from_env().and_then(|h| h.channel_location());
    let did = match location {
        Ok(Some(location)) => rekindle_release::update::run(
            &install,
            &location,
            &rekindle_release::signing::first_party_keys(),
            rekindle_plugin_host::updates::now(),
        )
        .map(|r| format!("{r:?}")),
        Ok(None) => Ok("no release channel".into()),
        Err(e) => Err(e),
    };
    did.unwrap_or_else(|e| {
        install.log(&format!("{e:#}"));
        format!("error: {e:#}")
    })
}
