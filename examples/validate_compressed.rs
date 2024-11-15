use ffbuildtool::Version;

use log::*;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest_file(manifest_path).unwrap();

    let asset_root_good = "example_builds/compressed/good/";
    let time = std::time::Instant::now();
    let corrupted = version
        .validate_compressed(asset_root_good, None)
        .await
        .unwrap();
    info!("Validation took {:?}", time.elapsed());
    assert!(corrupted.is_empty());

    let asset_root_bad = "example_builds/compressed/bad/";
    let time = std::time::Instant::now();
    let corrupted = version
        .validate_compressed(asset_root_bad, None)
        .await
        .unwrap();
    info!(
        "Validation took {:?}; corrupted files: {:?}",
        time.elapsed(),
        corrupted
    );
}
