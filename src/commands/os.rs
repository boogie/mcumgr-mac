//! OS-group commands: `echo` and `reset`.

use anyhow::{Context, Result};
use console::style;

use mcumgr_mac::smp::groups::{group, os};
use mcumgr_mac::smp::messages::{self, EchoRequest, EchoResponse};
use mcumgr_mac::smp::Op;

use crate::cli::GlobalOpts;
use crate::commands::open_session;
use crate::ui;

/// Send an echo request and print the device's reply.
pub async fn echo(global: &GlobalOpts, text: &str) -> Result<()> {
    let mut session = open_session(global).await?;

    let payload = messages::encode(&EchoRequest { d: text })?;
    let response = session
        .request(Op::Write, group::OS, os::ECHO, &payload)
        .await?;
    messages::check_rc(&response)?;
    let echoed: EchoResponse = messages::decode(&response).context("decoding echo response")?;

    ui::success(format!("Device echoed: {}", style(&echoed.r).bold()));
    session.disconnect().await;
    Ok(())
}

/// Reset (reboot) the device.
pub async fn reset(global: &GlobalOpts) -> Result<()> {
    let mut session = open_session(global).await?;

    // The device reboots and drops the link, often before (or instead of)
    // sending a response, so we fire the request without awaiting a reply.
    session
        .send(Op::Write, group::OS, os::RESET, &[])
        .await
        .context("sending reset request")?;

    ui::success("Reset requested \u{2014} the device will reboot and disconnect.");
    session.disconnect().await;
    Ok(())
}
