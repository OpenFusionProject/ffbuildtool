use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use ffbuildtool::{ItemProgress, Version};

use log::*;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let version = Version::from_manifest_file("manifest_104.json").unwrap();
    let output_path = "example_builds/downloaded";

    let total_download_size = Arc::new(AtomicU64::new(0));
    let total_download_size_cb = total_download_size.clone();
    let progress_callback = move |_uuid: &Uuid, _name: &str, progress: ItemProgress| {
        if let ItemProgress::Downloading {
            bytes_downloaded,
            total_bytes,
        } = progress
        {
            if bytes_downloaded == total_bytes {
                total_download_size_cb.fetch_add(bytes_downloaded, Ordering::AcqRel);
            }
        }
    };

    let time = std::time::Instant::now();
    version
        .download_compressed(output_path, Some(Arc::new(progress_callback)))
        .await
        .unwrap();
    info!("Downloading and validation took {:?}", time.elapsed());

    let total_downloaded = total_download_size.load(Ordering::Acquire);

    // failed downloads may add up to the total download size
    assert!(version.get_total_compressed_size() <= total_downloaded);
    info!(
        "Total download size: {:.2} MB",
        total_downloaded as f64 / 1024.0 / 1024.0
    );
}
