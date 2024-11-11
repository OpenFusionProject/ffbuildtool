use crate::Version;

#[tokio::test]
async fn test_validate_compressed_good() {
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest(manifest_path).unwrap();

    let asset_root_good = "example_builds\\compressed\\good\\";
    let corrupted = version.validate_compressed(asset_root_good).await.unwrap();
    assert!(corrupted.is_empty());
}

#[tokio::test]
async fn test_validate_compressed_bad() {
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest(manifest_path).unwrap();

    let asset_root_bad = "example_builds\\compressed\\bad\\";
    let corrupted = version.validate_compressed(asset_root_bad).await.unwrap();
    assert_eq!(corrupted, vec!["Map_00_00.unity3d"]);
}

#[tokio::test]
async fn test_validate_uncompressed_good() {
    let asset_root = "example_builds\\uncompressed\\good\\";
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest(manifest_path).unwrap();

    let corrupted = version.validate_uncompressed(asset_root).await.unwrap();
    assert!(corrupted.is_empty());
}

#[tokio::test]
async fn test_validate_uncompressed_bad() {
    let asset_root_bad = "example_builds\\uncompressed\\bad\\";
    let manifest_path = "example_manifest.json";
    let version = Version::from_manifest(manifest_path).unwrap();

    let corrupted = version.validate_uncompressed(asset_root_bad).await.unwrap();
    assert_eq!(
        corrupted,
        vec!["DongResources_00_09.resourceFile/CustomAssetBundle-52625066c401043eda0a3d5088cda126"]
    );
}

#[tokio::test]
async fn test_generate_manifest() {
    let asset_root = "example_builds\\compressed\\good\\";
    let asset_url = "http://example.url/builds/example_build/";
    let main_path = format!("{}main.unity3d", asset_root);
    let main_url = format!("{}main.unity3d", asset_url);
    let description = Some("example-build");
    let parent = None;

    let mut version = Version::build(
        &main_path,
        &main_url,
        asset_root,
        asset_url,
        description,
        parent,
    )
    .await
    .unwrap();

    let example_manifest = "example_manifest.json";
    let example_version = Version::from_manifest(example_manifest).unwrap();
    version.uuid = example_version.uuid;
    assert_eq!(version, example_version);

    let corrupted = version.validate_compressed(asset_root).await.unwrap();
    assert!(corrupted.is_empty());
}
