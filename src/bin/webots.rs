use std::sync::Arc;

use hulk::{hardware::WebotsInterface, setup_logger, Runtime};
use tokio_util::sync::CancellationToken;

fn main() -> anyhow::Result<()> {
    setup_logger()?;
    let keep_running = CancellationToken::new();
    {
        let keep_running = keep_running.clone();
        ctrlc::set_handler(move || {
            keep_running.cancel();
        })?;
    }
    let hardware = Arc::new(WebotsInterface::new(keep_running.clone()));
    let runtime = Runtime::construct(hardware)?;
    runtime.run(keep_running)?;

    Ok(())
}
