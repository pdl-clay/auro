// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal app flow for the GitHub Releases channel (spec 0037).

use std::error::Error;
use std::path::{Path, PathBuf};

use auro::prelude::*;
use auro::update::{
    ApplyJournal, ChannelConfig, GithubChannel, Recovery, UpdateError,
};

#[derive(Default)]
struct Hello;

impl Render for Hello {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().gap_2().p_4().child("Update channel example")
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::current_dir()?;
    reconcile(&dir)?;
    if maybe_update(&dir)? {
        // The helper owns the journal lock now; exit so it can apply and relaunch.
        return Ok(());
    }
    App::new().root::<Hello>("main").run();
    Ok(())
}

fn reconcile(dir: &Path) -> Result<(), Box<dyn Error>> {
    let path = dir.join(".auro-updates").join("update.json");
    let journal = match ApplyJournal::load(&path) {
        Ok(journal) => journal,
        Err(UpdateError::JournalMissing { .. }) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    match journal.reconcile(env!("CARGO_PKG_VERSION"))? {
        Recovery::Complete => journal.finalize()?,
        Recovery::RetryApply | Recovery::Relaunch => {
            eprintln!("auro: update needs a restart helper; launch the app again")
        }
        Recovery::AwaitReboot => eprintln!("auro: installer requested a reboot"),
        Recovery::Failed | Recovery::RolledBack => {
            eprintln!("auro: update failed; journal retained for diagnosis")
        }
        Recovery::Prepared => eprintln!("auro: prepared journal has no apply kind"),
    }
    Ok(())
}

fn maybe_update(dir: &Path) -> Result<bool, Box<dyn Error>> {
    let config = Config::load(dir)?;
    let Some(settings) = config.updater.as_ref() else {
        return Ok(false);
    };
    let channel = GithubChannel::new(
        ChannelConfig::new(&settings.repository, &settings.public_key)
            .with_manifest_asset(&settings.manifest_asset)
            .with_interval_secs(settings.interval_secs)
            .with_check_on_start(settings.check_on_start),
        dir.join(".auro-updates"),
        &config.app.version,
    )?;
    let target = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?);
    Ok(channel.apply_if_due(target, None)?)
}
