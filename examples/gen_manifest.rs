use ffbuildtool::Version;

use log::*;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let asset_root = "example_builds/compressed/good/";
    let asset_url = "http://example.url/builds/example_build/";
    let name = Some("example-build");
    let description = Some("Example build");
    let uuid_104 = Uuid::parse_str("ec8063b2-54d4-4ee1-8d9e-381f5babd420").unwrap();
    let parent = Some(uuid_104);

    let time = std::time::Instant::now();
    let version = Version::build(asset_root, asset_url, name, description, parent)
        .await
        .unwrap();
    info!("Processing took {:?}", time.elapsed());

    let outfile = "manifest.json";
    version.export_manifest(outfile).unwrap();
    info!("Wrote manifest to {}", outfile);
}
