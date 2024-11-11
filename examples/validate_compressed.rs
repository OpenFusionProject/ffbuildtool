use ffbuildtool::Version;

use log::*;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let asset_root = "example_build\\";
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest(manifest_path).unwrap();

    let time = std::time::Instant::now();
    let corrupted = version.validate_compressed(asset_root).await.unwrap();
    info!("Validation took {:?}", time.elapsed());
    assert!(corrupted.is_empty());

    let asset_root_bad = "example_build_bad\\";
    let time = std::time::Instant::now();
    let corrupted = version.validate_compressed(asset_root_bad).await.unwrap();
    info!(
        "Validation took {:?}; corrupted files: {:?}",
        time.elapsed(),
        corrupted
    );
}
