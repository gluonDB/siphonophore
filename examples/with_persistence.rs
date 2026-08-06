//! Example with file-based persistence

use async_trait::async_trait;
use siphonophore::{BeforeCloseDirtyPayload, Hook, HookResult, OnLoadDocumentPayload, Server};
use std::fs;
use std::path::Path;

struct FileStorage {
    dir: String,
}

impl FileStorage {
    fn new(dir: &str) -> Self {
        fs::create_dir_all(dir).ok();
        Self {
            dir: dir.to_string(),
        }
    }

    fn path(&self, doc_id: &str) -> String {
        // Sanitize doc_id for filesystem
        let safe_id: String = doc_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("{}/{}.bin", self.dir, safe_id)
    }
}

#[async_trait]
impl Hook for FileStorage {
    async fn on_load_document(
        &self,
        p: OnLoadDocumentPayload<'_>,
    ) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
        let path = self.path(p.doc_id);
        if Path::new(&path).exists() {
            println!("Loading {}", p.doc_id);
            Ok(Some(fs::read(&path)?))
        } else {
            println!("New document: {}", p.doc_id);
            Ok(None)
        }
    }

    async fn before_close_dirty(&self, p: BeforeCloseDirtyPayload<'_>) -> HookResult {
        let path = self.path(p.doc_id);
        println!("Saving {} ({} bytes)", p.doc_id, p.state.len());
        fs::write(&path, p.state)?;
        Ok(())
    }

    fn after_unload_document(&self, doc_id: &str) {
        println!("Unloaded {}", doc_id);
    }
}

#[tokio::main]
async fn main() {
    println!("Siphonophore listening on 0.0.0.0:8080 with file persistence in ./data");

    Server::with_hooks(vec![Box::new(FileStorage::new("./data"))])
        .serve("0.0.0.0:8080")
        .await
        .unwrap();
}
