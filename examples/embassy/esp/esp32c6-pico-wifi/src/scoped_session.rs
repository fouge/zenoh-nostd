//! Hardware regression: local Zenoh state, one task, repeated open/drop cycles.
use core::{cell::Cell, future::pending};
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer, with_timeout};
use zenoh_examples_esp32c6_pico_wifi::{ENDPOINT, init_session_example};
use zenoh_nostd::session::{Endpoint, GetResponse, Session, ZSessionConfig, zenoh};

const CYCLES: u32 = 8;
const KEY: &str = "test/scoped-session";
const REPLY: &[u8] = b"scoped-reply";

pub async fn run(spawner: embassy_executor::Spawner) {
    // Only the HAL/network tasks retain their platform-owned static storage.
    // The Zenoh configuration (including its three-slot link manager) is local.
    let result = {
        let config = init_session_example(&spawner).await;
        exercise(&config, ENDPOINT).await
    };
    // Reaching this marker also proves the local configuration can be dropped.
    match result {
        Ok(()) => zenoh::info!("SCOPED SESSION PASS: 8 cycles; local config dropped"),
        Err(error) => zenoh::error!("SCOPED SESSION FAIL: {}", error),
    }
    loop {
        Timer::after_secs(1).await;
    }
}

async fn exercise<C: ZSessionConfig>(config: &C, address: &str) -> Result<(), &'static str> {
    let endpoint = Endpoint::try_from(address).map_err(|_| "invalid endpoint")?;
    let key = zenoh::keyexpr::new(KEY).map_err(|_| "invalid test key")?;
    for cycle in 0..CYCLES {
        let cancel = cycle % 2 == 1;
        // These cells precede the transport/session and are borrowed by callbacks.
        let replies = Cell::new(0u32);
        let wrong_reply = Cell::new(false);
        let started = Cell::new(false);
        {
            let mut transport = with_timeout(
                Duration::from_secs(30),
                config.transports().connect(endpoint.clone(), config.buff()),
            )
            .await
            .map_err(|_| "connection timed out")?
            .map_err(|_| "connection failed (or a previous link slot leaked)")?;
            let session = Session::<C>::new(&mut transport);
            let work = async {
                let responses = session
                    .get(key)
                    .timeout(core::time::Duration::from_secs(2))
                    .callback_sync(|response| match response {
                        GetResponse::Ok(sample) if sample.payload() == REPLY => {
                            replies.set(replies.get() + 1);
                        }
                        _ => wrong_reply.set(true),
                    })
                    .finish()
                    .await
                    .map_err(|_| "get failed")?;
                started.set(true);
                if cancel {
                    // The outer timeout cancels this future with a get handle.
                    // The whole session/transport scope is discarded afterward.
                    pending::<()>().await;
                }
                while !responses.is_complete().await {
                    Timer::after_millis(10).await;
                }
                responses.cancel().await.map_err(|_| "get cleanup failed")?;
                if replies.get() != 1 || wrong_reply.get() {
                    return Err("expected one scoped-reply from the host queryable");
                }
                session.flush().await.map_err(|_| "flush failed")?;
                Ok(())
            };
            let timeout = if cancel {
                Duration::from_millis(250)
            } else {
                Duration::from_secs(5)
            };
            // Both futures run in this task. No spawned session driver, static
            // session/configuration, session! macro, or leaked allocation.
            match select(session.run(), with_timeout(timeout, work)).await {
                Either::First(_) => return Err("session driver ended early"),
                Either::Second(Ok(result)) => result?,
                Either::Second(Err(_)) if cancel && started.get() => {}
                Either::Second(Err(_)) => return Err("operation timed out"),
            }
        }
        // Session/driver has dropped before the transport; its pool slot is free.
        zenoh::info!(
            "SCOPED SESSION cycle={} mode={} complete",
            cycle + 1,
            if cancel { "cancel" } else { "reply" }
        );
        if cycle + 1 < CYCLES {
            // Dropping an Embassy socket releases its local buffers but does not
            // send a Zenoh Close. Let the peer retire the previous connection's
            // ten-second lease before reopening with the same Zenoh identity.
            Timer::after_secs(11).await;
        }
    }
    Ok(())
}
