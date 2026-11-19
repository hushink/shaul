//! Register a custom archive extractor.
//!
//! The toy `.demo` format below is plain text. A real 7z/rar/xz implementation
//! would use a third-party crate or an external tool, but the registry wiring
//! is identical. Custom extractors must keep every write under
//! `ExtractorRequest::extract_root` and must validate entry paths.
//!
//! ```console
//! cargo run -p shaul --example custom_extractor
//! ```

use std::path::Path;
use std::sync::Arc;

use shaul::{
    ArchiveExtractor, ExtractReport, ExtractorRegistry, ExtractorRequest, FinalizerPipeline,
    Result, sanitize_file_name,
};

struct DemoExtractor;

impl ArchiveExtractor for DemoExtractor {
    fn name(&self) -> &'static str {
        "demo"
    }

    fn supports(&self, path: &Path) -> Result<bool> {
        Ok(path.extension().and_then(|extension| extension.to_str()) == Some("demo"))
    }

    fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
        let stem = request
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("demo");
        let target = request.extract_root.join(sanitize_file_name(stem));
        std::fs::create_dir_all(&target)?;
        std::fs::copy(request.path, target.join("payload.txt"))?;
        Ok(ExtractReport {
            target,
            files_extracted: 1,
            warnings: Vec::new(),
        })
    }
}

fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let archive = dir.path().join("hello.demo");
    std::fs::write(&archive, b"hello from a custom extractor")?;

    // Custom extractors are tried before the built-in formats.
    let registry = Arc::new(ExtractorRegistry::default().with(Arc::new(DemoExtractor)));
    let report = registry
        .extract(&archive, dir.path(), false)?
        .expect("the demo extractor should claim .demo files");
    println!(
        "extracted {} file(s) to {}",
        report.files_extracted,
        report.target.display()
    );
    println!(
        "{}",
        std::fs::read_to_string(report.target.join("payload.txt"))?
    );

    // Use the same registry for a real engine:
    // let engine = engine.with_finalizers(pipeline);
    let _pipeline = FinalizerPipeline::with_extractor_registry(registry);
    Ok(())
}
