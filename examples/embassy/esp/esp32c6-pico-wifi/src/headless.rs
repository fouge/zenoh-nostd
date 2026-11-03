#[cfg(not(feature = "scoped-session-test"))]
use static_cell::StaticCell;
#[cfg(not(feature = "scoped-session-test"))]
use zenoh_examples_common::scenarios::ReportFn;
#[cfg(not(feature = "scoped-session-test"))]
use zenoh_examples_esp32c6_pico_wifi::*;

#[cfg(feature = "scoped-session-test")]
mod scoped_session;

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    #[cfg(feature = "scoped-session-test")]
    scoped_session::run(spawner).await;

    #[cfg(not(feature = "scoped-session-test"))]
    {
        static CONFIG: StaticCell<ExampleConfig> = StaticCell::new();
        static REPORT: ReportFn =
            ReportFn(|text| zenoh_nostd::session::zenoh::info!("{}", text.trim_end()));
        let config = CONFIG.init(init_session_example(&spawner).await);
        let _ = zenoh_examples_embassy::headless::run(config, ENDPOINT, &REPORT).await;
    }
}
