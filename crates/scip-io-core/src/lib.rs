pub mod config;
pub mod detect;
pub mod indexer;
pub mod merge;
pub mod progress;
pub mod validate;

// Re-export key types for convenience
pub use config::ProjectConfig;
pub use detect::languages::LanguageKind;
pub use detect::Language;
pub use progress::{MergeStats, ProgressEvent, ProgressHandler};
pub use validate::ValidationResult;
