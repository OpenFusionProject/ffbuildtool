use ffbuildtool::Version;

use log::*;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let asset_root = "example_builds\\compressed\\good\\";
    let asset_url = "http://example.url/builds/example_build/";
    let description = Some("example-build");
    let parent = None;

    let time = std::time::Instant::now();
    let version = Version::build(asset_root, asset_url, description, parent)
        .await
        .unwrap();
    info!("Processing took {:?}", time.elapsed());

    let outfile = "manifest.json";
    version.export_manifest(outfile).unwrap();
    info!("Wrote manifest to {}", outfile);
}
