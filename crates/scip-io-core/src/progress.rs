use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProgressEvent {
    DetectStart {
        path: PathBuf,
    },
    DetectResult {
        languages: Vec<String>,
    },
    DownloadStart {
        indexer: String,
        version: String,
    },
    DownloadProgress {
        indexer: String,
        bytes: u64,
        total: Option<u64>,
    },
    DownloadComplete {
        indexer: String,
        path: PathBuf,
    },
    IndexerStart {
        language: String,
        command: String,
    },
    IndexerOutput {
        language: String,
        line: String,
    },
    IndexerComplete {
        language: String,
        duration_secs: f64,
        output: PathBuf,
    },
    IndexerFailed {
        language: String,
        error: String,
    },
    MergeStart {
        inputs: Vec<PathBuf>,
    },
    MergeComplete {
        output: PathBuf,
        stats: MergeStats,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeStats {
    pub documents: usize,
    pub symbols: usize,
    pub size_bytes: u64,
}

pub trait ProgressHandler: Send + Sync {
    fn on_event(&self, event: ProgressEvent);
}

/// No-op handler for when progress reporting isn't needed.
pub struct NoopHandler;

impl ProgressHandler for NoopHandler {
    fn on_event(&self, _event: ProgressEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct TestHandler {
        events: Arc<Mutex<Vec<ProgressEvent>>>,
    }

    impl ProgressHandler for TestHandler {
        fn on_event(&self, event: ProgressEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[test]
    fn test_noop_handler_does_not_panic() {
        let handler = NoopHandler;
        handler.on_event(ProgressEvent::DetectStart {
            path: PathBuf::from("."),
        });
        handler.on_event(ProgressEvent::MergeStart {
            inputs: vec![PathBuf::from("a.scip")],
        });
    }

    #[test]
    fn test_custom_handler_receives_events() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let handler = TestHandler {
            events: events.clone(),
        };
        handler.on_event(ProgressEvent::DetectStart {
            path: PathBuf::from("/test"),
        });
        handler.on_event(ProgressEvent::DetectResult {
            languages: vec!["rust".into()],
        });
        assert_eq!(events.lock().unwrap().len(), 2);
    }

    #[test]
    fn test_custom_handler_preserves_event_order() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let handler = TestHandler {
            events: events.clone(),
        };
        handler.on_event(ProgressEvent::DownloadStart {
            indexer: "scip-typescript".into(),
            version: "0.3.11".into(),
        });
        handler.on_event(ProgressEvent::DownloadProgress {
            indexer: "scip-typescript".into(),
            bytes: 1024,
            total: Some(2048),
        });
        handler.on_event(ProgressEvent::DownloadComplete {
            indexer: "scip-typescript".into(),
            path: PathBuf::from("/bin/scip-typescript"),
        });
        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 3);
        // Verify first event is DownloadStart
        assert!(matches!(&captured[0], ProgressEvent::DownloadStart { .. }));
        assert!(matches!(
            &captured[2],
            ProgressEvent::DownloadComplete { .. }
        ));
    }

    #[test]
    fn test_progress_event_detect_serialization() {
        let event = ProgressEvent::DetectStart {
            path: PathBuf::from("/project"),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("DetectStart"));
        let deserialized: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, ProgressEvent::DetectStart { .. }));
    }

    #[test]
    fn test_progress_event_merge_complete_serialization() {
        let event = ProgressEvent::MergeComplete {
            output: PathBuf::from("index.scip"),
            stats: MergeStats {
                documents: 10,
                symbols: 100,
                size_bytes: 5000,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("MergeComplete"));
        assert!(json.contains("\"documents\":10"));
        assert!(json.contains("\"symbols\":100"));
        assert!(json.contains("\"size_bytes\":5000"));
    }

    #[test]
    fn test_progress_event_indexer_failed_serialization() {
        let event = ProgressEvent::IndexerFailed {
            language: "python".into(),
            error: "binary not found".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ProgressEvent = serde_json::from_str(&json).unwrap();
        match deserialized {
            ProgressEvent::IndexerFailed { language, error } => {
                assert_eq!(language, "python");
                assert_eq!(error, "binary not found");
            }
            _ => panic!("Wrong variant after deserialization"),
        }
    }

    #[test]
    fn test_merge_stats_serialization() {
        let stats = MergeStats {
            documents: 42,
            symbols: 1000,
            size_bytes: 123456,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: MergeStats = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.documents, 42);
        assert_eq!(deserialized.symbols, 1000);
        assert_eq!(deserialized.size_bytes, 123456);
    }

    #[test]
    fn test_all_event_variants_serialize() {
        // Ensure every variant can round-trip through JSON
        let events: Vec<ProgressEvent> = vec![
            ProgressEvent::DetectStart {
                path: PathBuf::from("."),
            },
            ProgressEvent::DetectResult {
                languages: vec!["rust".into()],
            },
            ProgressEvent::DownloadStart {
                indexer: "x".into(),
                version: "1".into(),
            },
            ProgressEvent::DownloadProgress {
                indexer: "x".into(),
                bytes: 0,
                total: None,
            },
            ProgressEvent::DownloadComplete {
                indexer: "x".into(),
                path: PathBuf::from("."),
            },
            ProgressEvent::IndexerStart {
                language: "rust".into(),
                command: "cmd".into(),
            },
            ProgressEvent::IndexerOutput {
                language: "rust".into(),
                line: "ok".into(),
            },
            ProgressEvent::IndexerComplete {
                language: "rust".into(),
                duration_secs: 1.5,
                output: PathBuf::from("index.scip"),
            },
            ProgressEvent::IndexerFailed {
                language: "rust".into(),
                error: "err".into(),
            },
            ProgressEvent::MergeStart { inputs: vec![] },
            ProgressEvent::MergeComplete {
                output: PathBuf::from("out.scip"),
                stats: MergeStats {
                    documents: 0,
                    symbols: 0,
                    size_bytes: 0,
                },
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let _: ProgressEvent = serde_json::from_str(&json).unwrap();
        }
    }
}
