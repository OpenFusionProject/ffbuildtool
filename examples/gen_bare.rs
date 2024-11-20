use ffbuildtool::Version;

use log::*;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let asset_url = "http://example.url/builds/example_build/";
    let description = Some("example-build");
    let version = Version::build_barebones(asset_url, description);

    let outfile = "manifest.json";
    version.export_manifest(outfile).unwrap();
    info!("Wrote barebones manifest to {}", outfile);
}
