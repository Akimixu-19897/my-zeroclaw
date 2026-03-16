#[allow(unused_imports)]
use super::runtime::dispatch::*;
#[allow(unused_imports)]
use super::runtime::keys::*;
#[allow(unused_imports)]
use super::runtime::lifecycle::*;
#[allow(unused_imports)]
use super::runtime::processing::*;
#[allow(unused_imports)]
use super::runtime::processing_approval::*;
#[allow(unused_imports)]
use super::runtime::processing_support::*;
#[allow(unused_imports)]
use super::runtime::prompt::*;
#[allow(unused_imports)]
use super::runtime::registry::*;
#[allow(unused_imports)]
use super::runtime::routing::*;
use super::*;
use crate::memory::{Memory, MemoryCategory, SqliteMemory};
use crate::observability::NoopObserver;
use crate::providers::{ChatMessage, Provider};
use crate::tools::{Tool, ToolResult};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

fn make_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("SOUL.md"), "# Soul\nBe helpful.").unwrap();
    std::fs::write(tmp.path().join("IDENTITY.md"), "# Identity\nName: ZeroClaw").unwrap();
    std::fs::write(tmp.path().join("USER.md"), "# User\nName: Test User").unwrap();
    std::fs::write(
        tmp.path().join("AGENTS.md"),
        "# Agents\nFollow instructions.",
    )
    .unwrap();
    std::fs::write(tmp.path().join("TOOLS.md"), "# Tools\nUse shell carefully.").unwrap();
    std::fs::write(
        tmp.path().join("HEARTBEAT.md"),
        "# Heartbeat\nCheck status.",
    )
    .unwrap();
    std::fs::write(tmp.path().join("MEMORY.md"), "# Memory\nUser likes Rust.").unwrap();
    tmp
}

mod core {
    use super::*;
    include!("core.rs");
}

mod support_channels {
    use super::*;
    include!("support_channels.rs");
}

mod support_providers {
    use super::*;
    include!("support_providers.rs");
}

use support_channels::*;
use support_providers::*;

struct DummyProvider;

#[async_trait::async_trait]
impl Provider for DummyProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        Ok("ok".to_string())
    }
}

pub(super) struct NoopMemory;

#[async_trait::async_trait]
impl Memory for NoopMemory {
    fn name(&self) -> &str {
        "noop"
    }

    async fn store(
        &self,
        _key: &str,
        _content: &str,
        _category: crate::memory::MemoryCategory,
        _session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::memory::MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn get(&self, _key: &str) -> anyhow::Result<Option<crate::memory::MemoryEntry>> {
        Ok(None)
    }

    async fn list(
        &self,
        _category: Option<&crate::memory::MemoryCategory>,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::memory::MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn count(&self) -> anyhow::Result<usize> {
        Ok(0)
    }

    async fn health_check(&self) -> bool {
        true
    }
}

pub(super) struct RecallMemory;

#[async_trait::async_trait]
impl Memory for RecallMemory {
    fn name(&self) -> &str {
        "recall-memory"
    }

    async fn store(
        &self,
        _key: &str,
        _content: &str,
        _category: crate::memory::MemoryCategory,
        _session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::memory::MemoryEntry>> {
        Ok(vec![crate::memory::MemoryEntry {
            id: "entry-1".to_string(),
            key: "memory_key_1".to_string(),
            content: "Age is 45".to_string(),
            category: crate::memory::MemoryCategory::Conversation,
            timestamp: "2026-02-20T00:00:00Z".to_string(),
            session_id: None,
            score: Some(0.9),
        }])
    }

    async fn get(&self, _key: &str) -> anyhow::Result<Option<crate::memory::MemoryEntry>> {
        Ok(None)
    }

    async fn list(
        &self,
        _category: Option<&crate::memory::MemoryCategory>,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::memory::MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn count(&self) -> anyhow::Result<usize> {
        Ok(1)
    }

    async fn health_check(&self) -> bool {
        true
    }
}

mod process_message_a {
    use super::*;
    include!("process_message_a.rs");
}

mod process_message_b {
    use super::*;
    include!("process_message_b.rs");
}

mod process_message_c {
    use super::*;
    include!("process_message_c.rs");
}

mod process_message_d {
    use super::*;
    include!("process_message_d.rs");
}

mod prompt_a {
    use super::*;
    include!("prompt_a.rs");
}

mod prompt_b {
    use super::*;
    include!("prompt_b.rs");
}

mod prompt_c {
    use super::*;
    include!("prompt_c.rs");
}

mod registry_a {
    use super::*;
    include!("registry_a.rs");
}

mod registry_b {
    use super::*;
    include!("registry_b.rs");
}

mod registry_c {
    use super::*;
    include!("registry_c.rs");
}
