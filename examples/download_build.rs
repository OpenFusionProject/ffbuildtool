use std::sync::atomic::{AtomicU64, Ordering};

use ffbuildtool::Version;

use log::*;

static TOTAL_DOWNLOAD_SIZE: AtomicU64 = AtomicU64::new(0);

fn download_callback(_name: &str, size: u64) {
    TOTAL_DOWNLOAD_SIZE.fetch_add(size, Ordering::SeqCst);
}

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let version = Version::from_manifest_file("manifest_104.json").unwrap();
    let output_path = "example_builds\\downloaded";

    let time = std::time::Instant::now();
    version
        .download_compressed(output_path, Some(download_callback))
        .await
        .unwrap();
    info!("Downloading and validation took {:?}", time.elapsed());

    let total_downloaded = TOTAL_DOWNLOAD_SIZE.load(Ordering::SeqCst);
    assert!(version.get_compressed_size() == total_downloaded);
    info!(
        "Total download size: {:.2} MB",
        total_downloaded as f64 / 1024.0 / 1024.0
    );
}
