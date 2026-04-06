use indicatif::{ProgressBar, ProgressStyle};
use scip_io_core::progress::{ProgressEvent, ProgressHandler};
use std::sync::Mutex;

/// CLI progress handler that renders indicatif progress bars.
pub struct CliProgressHandler {
    bar: Mutex<Option<ProgressBar>>,
}

impl CliProgressHandler {
    pub fn new() -> Self {
        Self {
            bar: Mutex::new(None),
        }
    }

    fn get_or_create_spinner(&self, msg: String) -> ProgressBar {
        let mut guard = self.bar.lock().unwrap();
        if let Some(ref pb) = *guard {
            pb.set_message(msg);
            pb.clone()
        } else {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.cyan} {msg}")
                    .unwrap(),
            );
            pb.set_message(msg);
            *guard = Some(pb.clone());
            pb
        }
    }

    fn finish_bar(&self) {
        let mut guard = self.bar.lock().unwrap();
        if let Some(pb) = guard.take() {
            pb.finish_and_clear();
        }
    }
}

impl ProgressHandler for CliProgressHandler {
    fn on_event(&self, event: ProgressEvent) {
        match event {
            ProgressEvent::DownloadStart { indexer, version } => {
                self.get_or_create_spinner(format!(
                    "Downloading {} v{}...",
                    indexer, version
                ));
            }
            ProgressEvent::DownloadProgress {
                indexer,
                bytes,
                total,
            } => {
                if let Some(total) = total {
                    let pct = (bytes as f64 / total as f64 * 100.0) as u64;
                    self.get_or_create_spinner(format!(
                        "Downloading {}... {}%",
                        indexer, pct
                    ));
                } else {
                    self.get_or_create_spinner(format!(
                        "Downloading {}... {} bytes",
                        indexer, bytes
                    ));
                }
            }
            ProgressEvent::DownloadComplete { indexer, path } => {
                self.finish_bar();
                tracing::debug!(indexer = %indexer, path = %path.display(), "download complete");
            }
            ProgressEvent::IndexerStart { language, command } => {
                self.get_or_create_spinner(format!(
                    "Running {} indexer ({})...",
                    language, command
                ));
            }
            ProgressEvent::IndexerOutput { language, line } => {
                tracing::debug!(language = %language, "{}", line);
            }
            ProgressEvent::IndexerComplete {
                language,
                duration_secs,
                output,
            } => {
                self.finish_bar();
                tracing::debug!(
                    language = %language,
                    duration = %duration_secs,
                    output = %output.display(),
                    "indexer complete"
                );
            }
            ProgressEvent::IndexerFailed { language, error } => {
                self.finish_bar();
                tracing::error!(language = %language, error = %error, "indexer failed");
            }
            ProgressEvent::DetectStart { path } => {
                tracing::debug!(path = %path.display(), "detect start");
            }
            ProgressEvent::DetectResult { languages } => {
                tracing::debug!(?languages, "detect result");
            }
            ProgressEvent::MergeStart { inputs } => {
                tracing::debug!(count = inputs.len(), "merge start");
            }
            ProgressEvent::MergeComplete { output, stats } => {
                tracing::debug!(
                    output = %output.display(),
                    documents = stats.documents,
                    symbols = stats.symbols,
                    size = stats.size_bytes,
                    "merge complete"
                );
            }
        }
    }
}
