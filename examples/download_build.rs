use ffbuildtool::Version;

use log::*;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let version = Version::from_manifest("manifest_104.json").unwrap();
    let output_path = format!("example_builds\\downloaded\\{}", version.get_uuid());

    let time = std::time::Instant::now();
    version.download_compressed(&output_path).await.unwrap();
    info!("Downloading and validation took {:?}", time.elapsed());
}
