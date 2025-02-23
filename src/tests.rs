use uuid::Uuid;

use crate::{util::TempDir, Version};

#[tokio::test]
async fn test_validate_compressed_good() {
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest_file(manifest_path).unwrap();

    let asset_root_good = "example_builds/compressed/good/";
    let corrupted = version
        .validate_compressed(asset_root_good, None)
        .await
        .unwrap();
    assert!(corrupted.is_empty());
}

#[tokio::test]
async fn test_validate_compressed_bad() {
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest_file(manifest_path).unwrap();

    let asset_root_bad = "example_builds/compressed/bad/";
    let corrupted = version
        .validate_compressed(asset_root_bad, None)
        .await
        .unwrap();
    assert_eq!(corrupted, vec!["Map_00_00.unity3d"]);
}

#[tokio::test]
async fn test_validate_uncompressed_good() {
    let asset_root = "example_builds/uncompressed/good/";
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest_file(manifest_path).unwrap();

    let corrupted = version
        .validate_uncompressed(asset_root, None)
        .await
        .unwrap();
    assert!(corrupted.is_empty());
}

#[tokio::test]
async fn test_validate_uncompressed_bad() {
    let asset_root_bad = "example_builds/uncompressed/bad/";
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest_file(manifest_path).unwrap();

    let corrupted = version
        .validate_uncompressed(asset_root_bad, None)
        .await
        .unwrap()
        .iter()
        .map(|x| x.to_ascii_lowercase())
        .collect::<Vec<_>>();

    assert_eq!(
        corrupted,
        vec!["DongResources_5f00_5f09_2eresourceFile/CustomAssetBundle-52625066c401043eda0a3d5088cda126".to_ascii_lowercase()]
    );
}

#[tokio::test]
async fn test_generate_manifest() {
    let asset_root = "example_builds/compressed/good/";
    let asset_url = "http://example.url/builds/example_build/";
    let name = Some("example-build");
    let description = Some("Example build");
    let uuid_104 = Uuid::parse_str("ec8063b2-54d4-4ee1-8d9e-381f5babd420").unwrap();
    let parent = Some(uuid_104);

    let mut version = Version::build(asset_root, asset_url, name, description, parent)
        .await
        .unwrap();

    let example_manifest = "example_manifest.json";
    let example_version = Version::from_manifest_file(example_manifest).unwrap();
    version.uuid = example_version.uuid;
    assert_eq!(version, example_version);

    let corrupted = version.validate_compressed(asset_root, None).await.unwrap();
    assert!(corrupted.is_empty());
}

#[cfg(feature = "lzma")]
#[tokio::test]
async fn test_extract_bundle() {
    use crate::bundle::AssetBundle;

    let bundle_path = "example_builds/compressed/good/Map_00_00.unity3d";
    let output_dir = TempDir::new();

    let (_, bundle) = AssetBundle::from_file(bundle_path).unwrap();
    bundle.extract_files(output_dir.path()).unwrap();

    let version = Version::from_manifest_file("example_manifest.json").unwrap();
    let bundle_info = version.get_bundle("Map_00_00.unity3d").unwrap();

    let corrupted = bundle_info
        .validate_uncompressed(output_dir.path(), Some(version.get_uuid()), None)
        .unwrap();
    assert!(corrupted.is_empty());
}

#[cfg(feature = "lzma")]
#[tokio::test]
async fn test_repack_bundle() {
    use crate::bundle::AssetBundle;

    let bundle_path = "example_builds/compressed/good/Map_00_00.unity3d";
    let output_dir = TempDir::new();

    let (_, og_bundle) = AssetBundle::from_file(bundle_path).unwrap();
    og_bundle.extract_files(output_dir.path()).unwrap();

    let repacked_bundle = AssetBundle::from_directory(output_dir.path()).unwrap();
    assert!(og_bundle == repacked_bundle);
}

#[cfg(feature = "lzma")]
#[tokio::test]
async fn test_pack_bundle() {
    use crate::bundle::AssetBundle;

    let bundle_path = "example_builds/compressed/good/Map_00_00.unity3d";
    let unpacked_path = "example_builds/uncompressed/good/map_5f00_5f00_2eunity3d";

    let (_, og_bundle) = AssetBundle::from_file(bundle_path).unwrap();
    let packed_bundle = AssetBundle::from_directory(unpacked_path).unwrap();
    assert!(og_bundle == packed_bundle);
}
