use tracing_appender::non_blocking;
use tracing_appender::rolling;
use tracing_subscriber;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub fn setup_basic_logging(dir: &str, suffix: &str) {
    let file_appender = rolling::Builder::new()
        .rotation(rolling::Rotation::DAILY)
        .filename_suffix(format!("{}.log", suffix))
        .build(dir)
        .unwrap();
    let error_appender = rolling::Builder::new()
        .rotation(rolling::Rotation::DAILY)
        .filename_suffix(format!("{}.error.log", suffix))
        .build(dir)
        .unwrap();

    let (file_writer, _guard) = non_blocking(file_appender);
    let (error_writer, _error_guard) = non_blocking(error_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_ansi(true)
        .with_filter(tracing_subscriber::filter::LevelFilter::INFO);

    let error_layer = tracing_subscriber::fmt::layer()
        .with_writer(error_writer)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_ansi(true)
        .with_filter(tracing_subscriber::filter::LevelFilter::WARN);

    tracing_subscriber::registry()
        .with(file_layer)
        .with(error_layer)
        .init();

    // Keep guards alive for the duration of the program
    std::mem::forget(_guard);
    std::mem::forget(_error_guard);
}
