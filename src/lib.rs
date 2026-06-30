// BL1NK Execution Engine
pub mod blink;

// Shared Configuration
pub mod config;

// AI Providers
pub mod providers;

// Git Operations
pub mod git;

// Error Handling
pub mod errors;

// Re-exports for convenience
pub use blink::SequentialExecutor;
pub use config::ConfigManager;
pub use providers::ProviderFactory;
pub use errors::{BlinkError, Result};

// ============================================
// Module Documentation
// ============================================

/// BL1NK Autonomous Execution Engine
/// 
/// Provides sequential task execution through multiple AI providers
/// with state management and sandboxed git environments.
///
/// # Example
/// ```ignore
/// use blink_nexus::{SequentialExecutor, ConfigManager};
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let config = ConfigManager::load().await?;
///     let executor = SequentialExecutor::new(config)?;
///     executor.execute("Build a REST API", None).await?;
///     Ok(())
/// }
/// ```