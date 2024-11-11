use ffbuildtool::Version;

use log::*;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let asset_root = "example_build\\";
    let asset_url = "http://example.url/builds/example_build/";
    let main_path = format!("{}main.unity3d", asset_root);
    let main_url = format!("{}main.unity3d", asset_url);
    let description = Some("example-build");
    let parent = None;

    let time = std::time::Instant::now();
    let version = Version::build(
        &main_path,
        &main_url,
        asset_root,
        asset_url,
        description,
        parent,
    )
    .await
    .unwrap();
    info!("Processing took {:?}", time.elapsed());

    let outfile = "manifest.json";
    version.export_manifest(outfile).unwrap();
    info!("Wrote manifest to {}", outfile);
}
