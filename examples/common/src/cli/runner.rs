use super::*;
use crate::scenarios::operation;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, with_timeout};
use zenoh_nostd::session::ZSessionConfig;
use zenoh_nostd::session::{Endpoint, Session};

async fn next(cli: &Cli) -> Result<Command, i32> {
    if cli.shutdown.get() {
        return Err(0);
    }
    let initial = cli.initial.borrow_mut().take();
    if let Some(line) = initial {
        match frontend::parse_line(&line) {
            Ok(submission) => {
                cli.bytes(&submission.output).await;
                return submission
                    .command
                    .ok_or(if submission.invalid { 2 } else { 0 });
            }
            Err(error) => {
                cli.report(format_args!("error: {error}\r\n")).await;
                return Err(2);
            }
        }
    }
    Ok(cli.commands.receive().await)
}

pub async fn application<C: AppConfig + 'static>(
    config: &'static C,
    cli: &'static Cli,
    default_endpoint: &str,
    can_listen: bool,
) -> i32 {
    let mut slot = core::pin::pin!(zenoh_nostd::session::SessionSlot::<C>::new());
    loop {
        let cmd = match next(cli).await {
            Ok(cmd) => cmd,
            Err(code) => return code,
        };
        match cmd.kind {
            Kind::Status => {
                cli.status();
                if cli.one_shot.get() {
                    return 0;
                }
                cli.prompt().await;
                continue;
            }
            Kind::Quit => return 0,
            Kind::Close => {
                cli.text("session already closed\r\n").await;
                if cli.one_shot.get() {
                    return 0;
                }
                cli.prompt().await;
                continue;
            }
            _ => {}
        }
        if cmd.listen && !can_listen {
            cli.text("error: listening is unsupported on this platform\r\n")
                .await;
            if cli.one_shot.get() {
                return 2;
            }
            cli.prompt().await;
            continue;
        }
        let address = if cmd.endpoint.is_empty() {
            default_endpoint
        } else {
            cmd.endpoint.as_str()
        };
        if address.is_empty() {
            cli.text("error: set ENDPOINT or use z_open <endpoint>\r\n")
                .await;
            if cli.one_shot.get() {
                return 2;
            }
            cli.prompt().await;
            continue;
        }
        let endpoint = match Endpoint::try_from(address) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                cli.report(format_args!("error: {error}\r\n")).await;
                if cli.one_shot.get() {
                    return 2;
                }
                cli.prompt().await;
                continue;
            }
        };
        cli.reset_command();
        if cmd.kind == Kind::Broker {
            #[cfg(feature = "alloc")]
            {
                if can_listen {
                    let options = cmd.scenario().expect("broker scenario");
                    let run = crate::scenarios::broker(
                        config,
                        endpoint,
                        options.listen1,
                        options.listen2,
                        cli,
                    );
                    let code = match select(cli.cancel.wait(), run).await {
                        Either::First(_) => {
                            cli.text("broker stopped\r\n").await;
                            0
                        }
                        Either::Second(Ok(())) => 0,
                        Either::Second(Err(error)) => {
                            cli.report(format_args!("broker error: {error}\r\n")).await;
                            1
                        }
                    };
                    if cli.one_shot.get() {
                        return code;
                    }
                } else {
                    cli.text("error: broker listening is unsupported on this platform\r\n")
                        .await;
                    if cli.one_shot.get() {
                        return 2;
                    }
                }
            }
            #[cfg(not(feature = "alloc"))]
            {
                cli.text("error: z_broker requires --features alloc\r\n")
                    .await;
                if cli.one_shot.get() {
                    return 2;
                }
            }
            cli.prompt().await;
            continue;
        }
        cli.report(format_args!("opening {address}\r\n")).await;
        // Scope the transport and session to this connection. No reusable static
        // cell or lifetime extension is needed, including after cancellation.
        let open = async {
            if cmd.listen {
                ZSessionConfig::transports(config)
                    .listen(endpoint, ZSessionConfig::buff(config))
                    .await
            } else {
                ZSessionConfig::transports(config)
                    .connect(endpoint, ZSessionConfig::buff(config))
                    .await
            }
        };
        let transport = match select(
            cli.cancel.wait(),
            with_timeout(Duration::from_secs(30), open),
        )
        .await
        {
            Either::First(_) => {
                cli.text("connection cancelled\r\n").await;
                if cli.one_shot.get() {
                    return 130;
                }
                cli.prompt().await;
                continue;
            }
            Either::Second(Err(_)) => {
                cli.text("error: connection timed out\r\n").await;
                if cli.one_shot.get() {
                    return 1;
                }
                cli.prompt().await;
                continue;
            }
            Either::Second(Ok(Err(error))) => {
                cli.report(format_args!("connection error: {error}\r\n"))
                    .await;
                if cli.one_shot.get() {
                    return 1;
                }
                cli.prompt().await;
                continue;
            }
            Either::Second(Ok(Ok(transport))) => transport,
        };
        let guard = slot.as_mut().insert(transport);
        let session = guard.session();
        cli.connected.set(true);
        cli.text("session open\r\n").await;
        let first = if cmd.kind == Kind::Open {
            None
        } else {
            Some(&cmd)
        };
        let exit = match select(session.run(), connected(session, cli, first)).await {
            Either::First(Err(error)) => {
                cli.report(format_args!("session error: {error}\r\n")).await;
                Some(1)
            }
            Either::First(Ok(())) => Some(0),
            Either::Second(exit) => exit,
        };
        // Both futures and all callbacks are dropped before transport storage.
        cli.connected.set(false);
        if let Some(code) = exit
            && (cli.one_shot.get() || cli.shutdown.get() || code == 0)
        {
            return code;
        }
        cli.text("session closed\r\n").await;
        cli.prompt().await;
    }
}

async fn connected<C: AppConfig>(
    session: &Session<'_, C>,
    cli: &'static Cli,
    first: Option<&Command>,
) -> Option<i32> {
    if let Some(cmd) = first {
        let code = execute(session, cli, cmd).await;
        if cli.one_shot.get() {
            return Some(code);
        }
        if code == 1 {
            return None;
        }
    } else if cli.one_shot.get() {
        return Some(0);
    }
    cli.prompt().await;
    loop {
        let cmd = match next(cli).await {
            Ok(cmd) => cmd,
            Err(code) => return Some(code),
        };
        match cmd.kind {
            Kind::Close => return None,
            Kind::Quit => return Some(0),
            Kind::Open => {
                cli.text("session already open; z_close before changing endpoint\r\n")
                    .await;
            }
            Kind::Broker => {
                cli.text("error: z_close before starting exclusive broker mode\r\n")
                    .await;
            }
            Kind::Status => cli.status(),
            _ => {
                if execute(session, cli, &cmd).await == 1 {
                    return None;
                }
            }
        }
        if cli.shutdown.get() {
            return Some(0);
        }
        cli.prompt().await;
    }
}

async fn execute<C: AppConfig>(session: &Session<'_, C>, cli: &'static Cli, cmd: &Command) -> i32 {
    cli.reset_command();
    let options = cmd.scenario().expect("operation scenario");
    let code = match select(
        cli.cancel.wait(),
        operation(session, &cli.scenario, &options, cli),
    )
    .await
    {
        Either::First(_) => {
            cli.text("cancelled\r\n").await;
            130
        }
        Either::Second(Ok(())) => {
            cli.text("done\r\n").await;
            0
        }
        Either::Second(Err(error)) => {
            cli.report(format_args!("command error: {error}\r\n")).await;
            1
        }
    };
    // Cancellation can interrupt registration or I/O. Remove every registration
    // before accepting another command; close the scoped session if this fails.
    match with_timeout(Duration::from_secs(2), session.clear_declarations()).await {
        Ok(Ok(())) => {}
        _ => {
            cli.text("cleanup failed; closing session\r\n").await;
            return 1;
        }
    }
    cli.scenario.reset();
    code
}
