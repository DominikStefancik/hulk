use std::str::FromStr;

use clap::Parser;
use color_eyre::{eyre::bail, Result};
use communication::{
    client::{Communication, CyclerOutput, SubscriberMessage},
    messages::Format,
};
use log::{error, info};

use crate::logging::setup_logger;

mod logging;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct CommandlineArguments {
    #[clap(short, long, default_value = "localhost")]
    address: String,
    path: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logger()?;

    // create a subscriber for logs used by "tracing" crate
    // for emitting log messages, see crates/control/src/path_planner.rs, line 318
    let subscriber = tracing_subscriber::fmt()
        // Use a more compact, abbreviated log format
        .compact()
        // Display source code file paths
        .with_file(true)
        // Display source code line numbers
        .with_line_number(true)
        // Display the thread ID an event was recorded on
        .with_thread_ids(true)
        // Don't display the event's target (module path)
        .with_target(false)
        // Build the subscriber
        .finish();

    tracing::subscriber::set_global_default(subscriber)?;

    let arguments = CommandlineArguments::parse();
    let output_to_subscribe = CyclerOutput::from_str(&arguments.path)?;
    let communication = Communication::new(Some(format!("ws://{}:1337", arguments.address)), true);
    let (_uuid, mut receiver) = communication
        .subscribe_output(output_to_subscribe, Format::Textual)
        .await;
    while let Some(message) = receiver.recv().await {
        match message {
            SubscriberMessage::Update { value } => println!("{value:#}"),
            SubscriberMessage::SubscriptionSuccess => info!("Successfully subscribed"),
            SubscriberMessage::SubscriptionFailure { info } => {
                error!("Failed to subscribe: {info:?}");
                break;
            }
            SubscriberMessage::UpdateBinary { .. } => bail!("Cannot print binary data"),
        }
    }
    Ok(())
}
