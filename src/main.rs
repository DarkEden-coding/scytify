//! Process startup and shutdown; playback and window state belong to their respective modules.
use iced::futures::FutureExt;
use scytify::{backend, contracts, desktop, frontend, migrate};
use std::{panic::AssertUnwindSafe, time::Duration};

/// Run the native window on the main thread and services on a small asynchronous runtime.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("off")).init();
    migrate::run();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let (frontend_port, backend_port) = contracts::channels(backend::initial_state());
    let commands = frontend_port.commands.clone();
    let errors = backend_port.events.clone();
    let desktop_commands = commands.clone();
    let desktop_events = backend_port.events.clone();
    let desktop_state = frontend_port.state.clone();
    let desktop = runtime.spawn(async move {
        if let Err(error) = desktop::run(
            desktop_commands.clone(),
            desktop_events.clone(),
            desktop_state,
        )
        .await
        {
            eprintln!("Desktop integration unavailable: {}", error.message);
            let _ = desktop_events.send(contracts::Event::Notice(error)).await;
        }
    });
    let icon = runtime.spawn(desktop::run_icon(frontend_port.state.clone()));
    let backend = runtime.spawn(async move {
        let result = AssertUnwindSafe(backend::run(backend_port)).catch_unwind().await;
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => error,
            Err(_) => contracts::AppError {
                kind: contracts::ErrorKind::Internal,
                message: "Backend task panicked; playback stopped and the last checkpoint may be incomplete.".into(),
                retry_after_seconds: None,
            },
        };
        eprintln!("Backend stopped: {}", error.message);
        let _ = errors.send(contracts::Event::Notice(error)).await;
    });
    let ui_result = {
        let _runtime_context = runtime.enter();
        frontend::run(frontend_port)
    };
    runtime.block_on(async {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            commands.send(contracts::Command {
                id: contracts::RequestId(u64::MAX),
                action: contracts::Action::Quit,
            }),
        )
        .await;
        let mut backend = backend;
        match tokio::time::timeout(Duration::from_secs(5), &mut backend).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("Backend task failed: {error}"),
            Err(_) => {
                eprintln!("Backend did not finish shutdown within five seconds");
                backend.abort();
            }
        }
    });
    desktop.abort();
    icon.abort();
    runtime.shutdown_timeout(Duration::from_secs(2));
    ui_result.map_err(Into::into)
}
