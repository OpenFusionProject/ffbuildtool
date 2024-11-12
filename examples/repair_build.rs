use std::path::PathBuf;

use ffbuildtool::{
    util::{self, TempDir},
    Version,
};

use log::*;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    let build_path = "example_builds\\compressed\\bad\\";
    let good_build_path = PathBuf::from("example_builds\\compressed\\good\\")
        .canonicalize()
        .unwrap();

    let mut version = Version::from_manifest("example_manifest.json").unwrap();
    let test_asset_url = util::file_path_to_uri(good_build_path.to_str().unwrap());
    version.set_asset_url(&test_asset_url);
    info!("Overrode asset URL: {}", test_asset_url);

    let tmp = TempDir::new();
    let new_path = tmp.path();
    util::copy_dir(build_path, new_path, true).unwrap();

    let time = std::time::Instant::now();
    version.repair(new_path).await.unwrap();
    info!("Repairing took {:?}", time.elapsed());
}
