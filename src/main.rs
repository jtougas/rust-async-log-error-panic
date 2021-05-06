use std::{future::Future, vec};
use std::panic;
use std::time::Instant;
use tracing::instrument;
use tracing_error::ErrorLayer;
use tracing_log::LogTracer;
use tracing_subscriber::{fmt, prelude::__tracing_subscriber_SubscriberExt, EnvFilter};

use eyre::Result;
use std::time::Duration;

fn init_log_system() -> Result<()> {
    // https://docs.rs/tracing
    // Lot's of useful examples in here: https://github.com/tokio-rs/tracing/tree/master/examples

    // convert all log messages to tracing events
    LogTracer::builder()
        .with_max_level(log::LevelFilter::Trace)
        .init()?;

    let env_filter_layer =
        EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info"))?;

    let subscriber = tracing_subscriber::registry()
        .with(ErrorLayer::default())
        .with(env_filter_layer)
        .with(
            fmt::layer()
                .with_thread_ids(true)
                .with_thread_names(true)
                .with_writer(|| std::io::stdout()),
        );

    tracing::subscriber::set_global_default(subscriber)?;

    // install panic and error report hooks
    let hook_builder = color_eyre::config::HookBuilder::default();
    let (panic_hook, eyre_hook) = hook_builder.into_hooks();
    eyre_hook.install()?;

    // replace the current panic hook with one that prints to log system
    std::panic::set_hook(Box::new(move |panic_info| {
        log::error!("{}", panic_hook.panic_report(panic_info));
    }));

    Ok(())
}

pub fn main() {
    init_log_system().unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = rt.block_on(spawn_tasks());
    drop(rt);
    match result {
        Ok(_) => {
            println!("=== Program finished with no errors ===");
            std::process::exit(0)
        }
        Err(_err) => {
            println!("=== Program finished with errors === {}", _err);
            std::process::exit(2)
        }
    }
}

fn panic_if_elapsed(id: &str, start: Instant, panic_at_ms: std::option::Option<u64>) {
    if let Some(ms) = panic_at_ms {
        if (ms as u128) <= start.elapsed().as_millis() {
            panic!("worker [{:?}] panic", &id);
        }
    }
}

fn error_if_elapsed(id: &str, start: Instant, error_at_ms: std::option::Option<u64>) -> Result<()> {
    if let Some(ms) = error_at_ms {
        if (ms as u128) <= start.elapsed().as_millis() {
            return Err(eyre::eyre!("worker [{:?}] error", &id));
        }
    }
    Ok(())
}

#[instrument]
async fn worker(
    id: &str,
    sleep1_ms: u64,
    sleep2_ms: u64,
    panic_at_ms: std::option::Option<u64>,
    error_at_ms: std::option::Option<u64>,
) -> Result<()> {
    use eyre::WrapErr;
    use color_eyre::{SectionExt, Section};
    let _guard = scopeguard::guard((), |_| {
        log::info!("worker [{:?}] dropped!", &id);
    });

    let start = Instant::now();

    log::info!("worker [{:?}] pre-sleep", &id);
    panic_if_elapsed(&id, start, panic_at_ms);
    error_if_elapsed(&id, start, error_at_ms).wrap_err("error at pre-sleep").with_section(|| "yo pre-sleep\nline2".header("### Dude ###"))?;

    tokio::time::sleep(Duration::from_millis(sleep1_ms)).await;
    log::info!("worker [{:?}] mid-sleep", &id);
    panic_if_elapsed(&id, start, panic_at_ms);
    error_if_elapsed(&id, start, error_at_ms).wrap_err("error at mid-sleep").with_section(|| "yo mid-sleep".header("Dude"))?;

    tokio::time::sleep(Duration::from_millis(sleep2_ms)).await;
    log::info!("worker [{:?}] post-sleep", &id);
    panic_if_elapsed(&id, start, panic_at_ms);
    error_if_elapsed(&id, start, error_at_ms).wrap_err("error at post-sleep").with_section(|| "yo post-sleep".header("Dude"))?;

    log::info!("worker [{:?}] finished", &id);
    Ok(())
}

async fn task_or_die<T: Future<Output = Result<()>>>(
    quit_sender: tokio::sync::broadcast::Sender<()>,
    mut quit_receiver: tokio::sync::broadcast::Receiver<()>,
    id: &str,
    future1: T,
) {
    tokio::select!(
        result = future1 => {

            // notify that we're quiting
            quit_sender.send(()).unwrap();


            match result {
                Ok(_ok) =>  log::error!("branch {:?} finished Ok", &id),
                Err(_err) =>  log::error!("branch {:?} finished Err {:?}", &id, _err),
            }
        }
        _ = quit_receiver.recv() => {}
    )
}

async fn task_or_die_spawn<T: Future<Output = Result<()>>>(
    quit_sender: tokio::sync::broadcast::Sender<()>,
    quit_receiver: tokio::sync::broadcast::Receiver<()>,
    id: &'static str,
    task: T,
) -> Result<()>
where
    T: Future + Send + 'static,
{
    use eyre::WrapErr;
    let result = tokio::spawn(task_or_die(quit_sender.clone(), quit_receiver, &id, task))
        .await
        .wrap_err("Spawned task failed");

    match result {
        Ok(_ok) => {
            log::info!("branch {:?} ok", &id);
        }
        Err(_err) => {
            log::error!("branch {:?} panic {:?}", &id, _err);
            // notify that we're quiting
            quit_sender.send(())?;
            return Err(eyre::eyre!(format!(
                "Spawned task for worker {:?} crashed",
                &id
            )));
        }
    }
    Ok(())
}

async fn spawn_tasks() -> Result<()> {
    let (quit_sender, _quit_receiver) = tokio::sync::broadcast::channel::<()>(1);

    let futures = vec![
        task_or_die_spawn(quit_sender.clone(), quit_sender.subscribe(), "1", worker("1", 2000, 2000, None, Some(0))),
        task_or_die_spawn(quit_sender.clone(), quit_sender.subscribe(), "2", worker("2", 2000, 2000, None, None)),
        task_or_die_spawn(quit_sender.clone(), quit_sender.subscribe(), "3", worker("3", 2000, 2000, None, None))
    ];

    futures::future::try_join_all(futures).await?;
    Ok(())
}
