use clap::Parser;
use std::error::Error;

pub mod app;
pub mod client;
pub mod fsm;
pub mod gui;
pub mod message;
pub mod renderer;
pub mod server;

#[derive(Parser)]
#[command(
    about = "Networked multiplayer game demo with client-server architecture. Run with GUI by default in headless server mode."
)]
struct Cli {
    #[arg(
        long,
        help = "Starts a server only in headless mode without graphical user interface. Used for creating dedicated servers."
    )]
    #[arg(long)]
    server_only: bool,

    #[arg(short, long)]
    port: u16,

    #[arg(long)]
    trace: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    if cli.trace {
        println!("Message tracking enabled");
        message::set_trace(true);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;

    if cli.server_only {
        //cargo run -- --port 8080 --server-only --trace

        print!("Starting server in headless mode");
        rt.block_on(async {
            match server::start_server(cli.port).await {
                Ok(_) => {
                    println!("Server started successfully. Press ctrl + C to shutdown the server");

                    match tokio::signal::ctrl_c().await {
                        Ok(_) => {
                            println!("\nCtrl + C signal received. Shutting down gracefully...")
                        }

                        Err(e) => eprint!("Failed to listen for ctrl + C"),
                    }
                }

                Err(e) => {
                    eprint!("Server failed to start: {}", e);

                    std::process::exit(1);
                }
            }
        })
    }

    // Run graphical client otherwise.
    app::run_app(&rt)
}
