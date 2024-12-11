use std::path::PathBuf;

use ffbuildtool::{bundle::AssetBundle, util, Version};

use log::*;

#[tokio::main]
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    #[cfg(not(feature = "lzma"))]
    error!("This example requires the lzma feature to be enabled");

    #[cfg(feature = "lzma")]
    {
        let bundle_name = "Map_01_03.unity3d";
        let asset_path = format!("example_builds/compressed/good/{}", bundle_name);
        let output_dir = "example_extracted/";
        std::fs::remove_dir_all(output_dir).ok();

        let time = std::time::Instant::now();
        let asset = AssetBundle::from_file(&asset_path).unwrap();
        asset.extract_files(output_dir).unwrap();
        info!("Extraction took {:?}", time.elapsed());

        let version = Version::from_manifest_file("example_manifest.json").unwrap();
        let bundle = version.get_bundle(bundle_name).unwrap();
        let bundle_name_url_encoded = util::url_encode(bundle_name);
        let bundle_root = PathBuf::from(output_dir).join(bundle_name_url_encoded);

        let time = std::time::Instant::now();
        bundle
            .validate_uncompressed(
                bundle_root.to_str().unwrap(),
                Some(version.get_uuid()),
                None,
            )
            .await
            .unwrap();
        info!("Validation took {:?}", time.elapsed());
    }
}
