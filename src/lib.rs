use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use bundle::AssetBundle;
use serde::{Deserialize, Serialize};
use tokio::{sync::Semaphore, task::JoinHandle};
use util::TempFile;
use uuid::Uuid;

use log::*;

pub type Error = Box<dyn std::error::Error>;

pub mod bundle;
pub mod util;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum FailReason {
    BadSize { expected: u64, actual: u64 },
    BadHash { expected: String, actual: String },
    Missing,
}
impl std::fmt::Display for FailReason {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FailReason::BadSize { expected, actual } => {
                write!(f, "Bad size: {} (disk) vs {} (manifest)", actual, expected)
            }
            FailReason::BadHash { expected, actual } => {
                write!(f, "Bad hash: {} (disk) vs {} (manifest)", actual, expected)
            }
            FailReason::Missing => write!(f, "File missing"),
        }
    }
}
impl std::error::Error for FailReason {}

#[derive(Debug)]
pub enum ItemProgress {
    Downloading {
        bytes_downloaded: u64,
        total_bytes: u64,
    },
    Validating,
    Passed {
        item_size: u64,
    },
    Failed {
        item_size: u64,
        reason: FailReason,
    },
}

// uuid, item name, progress
pub type ProgressCallback = Arc<dyn Fn(&Uuid, &str, ItemProgress) + Send + Sync>;

static ITEM_PERMITS: OnceLock<Semaphore> = OnceLock::new();
static DOWNLOAD_PERMITS: OnceLock<Semaphore> = OnceLock::new();

/// Sets the maximum number of concurrent items that can be processed at once for all operations.
/// Returns an error if the value has already been set.
pub fn set_max_concurrent_items(max: usize) -> Result<(), String> {
    ITEM_PERMITS
        .set(Semaphore::new(max))
        .map_err(|_| "Limit already set".to_string())
}

/// Sets the maximum number of concurrent downloads that can run at once for download/repair operations.
/// Returns an error if the value has already been set.
pub fn set_max_concurrent_downloads(max: usize) -> Result<(), String> {
    DOWNLOAD_PERMITS
        .set(Semaphore::new(max))
        .map_err(|_| "Limit already set".to_string())
}

/// Contains all the info comprising a FusionFall build.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Version {
    uuid: Uuid,
    asset_url: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    parent_uuid: Option<Uuid>,

    #[serde(skip_serializing_if = "Option::is_none")]
    hidden: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    main_file_url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    main_file_info: Option<FileInfo>,

    #[serde(skip_serializing_if = "Option::is_none")]
    total_compressed_size: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    total_uncompressed_size: Option<u64>,

    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    bundles: HashMap<String, BundleInfo>,
}
impl Version {
    /// Generates `Version` metadata given a local build root (compressed asset bundles).
    pub async fn build(
        asset_root: &str,
        asset_url: &str,
        name: Option<&str>,
        description: Option<&str>,
        parent: Option<Uuid>,
    ) -> Result<Self, Error> {
        let main_path = PathBuf::from(asset_root).join("main.unity3d");
        let main_file_info = FileInfo::build(&main_path.to_string_lossy()).await.ok();
        let (total_compressed_size, total_uncompressed_size, bundles) =
            Self::get_bundle_info(asset_root).await?;
        let asset_url = asset_url.trim_end_matches('/');
        let main_file_url = format!("{}/main.unity3d", asset_url);
        Ok(Self {
            uuid: Uuid::new_v4(),
            name: name.map(|s| s.to_string()),
            description: description.map(|s| s.to_string()),
            parent_uuid: parent,
            main_file_url: Some(main_file_url),
            main_file_info,
            hidden: Some(false),
            total_compressed_size: Some(total_compressed_size),
            total_uncompressed_size: Some(total_uncompressed_size),
            asset_url: asset_url.to_string(),
            bundles,
        })
    }

    // Generates barebones `Version` metadata with only the asset URL and optional name.
    pub fn build_barebones(asset_url: &str, name: Option<&str>) -> Self {
        Self {
            uuid: Uuid::new_v4(),
            name: name.map(|s| s.to_string()),
            description: None,
            parent_uuid: None,
            main_file_url: None,
            main_file_info: None,
            hidden: None,
            asset_url: asset_url.to_string(),
            total_compressed_size: None,
            total_uncompressed_size: None,
            bundles: HashMap::new(),
        }
    }

    pub fn get_uuid(&self) -> Uuid {
        self.uuid
    }

    pub fn get_description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the total size of the build in bytes, including the main file.
    pub fn get_total_compressed_size(&self) -> u64 {
        self.main_file_info.clone().unwrap_or_default().size + self.get_compressed_assets_size()
    }

    /// Returns the total size of the compressed asset bundles in bytes.
    pub fn get_compressed_assets_size(&self) -> u64 {
        self.total_compressed_size.unwrap_or(0)
    }

    /// Returns the total size of the uncompressed asset bundles in bytes.
    pub fn get_uncompressed_assets_size(&self) -> u64 {
        self.total_uncompressed_size.unwrap_or(0)
    }

    /// Returns the asset URL for the build without a trailing slash.
    pub fn get_asset_url(&self) -> String {
        let mut url = self.asset_url.clone();
        if url.ends_with('/') {
            url.pop();
        }
        url
    }

    /// Marks the build as hidden or unhidden. What this means is up to the client code;
    /// in OpenFusionLauncher, it will hide the build from the list of available builds.
    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = Some(hidden);
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden.unwrap_or(false)
    }

    /// Overrides the asset URL for the build. Useful for testing.
    pub fn set_asset_url(&mut self, asset_url: &str) {
        self.asset_url = asset_url.to_string();
    }

    /// Loads the `Version` metadata from a JSON manifest file path or URL.
    pub async fn from_manifest(path_or_url: &str) -> Result<Self, Error> {
        if path_or_url.starts_with("http") {
            Self::from_manifest_url(path_or_url).await
        } else {
            Self::from_manifest_file(path_or_url)
        }
    }

    /// Loads the `Version` metadata from a JSON manifest file.
    pub fn from_manifest_file(path: &str) -> Result<Self, Error> {
        let json = std::fs::read_to_string(path)?;
        let version: Self = serde_json::from_str(&json)?;
        Ok(version)
    }

    /// Loads the `Version` metadata from a JSON manifest file hosted on the web.
    pub async fn from_manifest_url(url: &str) -> Result<Self, Error> {
        let manifest = TempFile::download(url).await?;
        let version = Self::from_manifest_file(manifest.path())?;
        Ok(version)
    }

    /// Exports the `Version` metadata to a JSON file to be served from an API server.
    pub fn export_manifest(&self, path: &str) -> Result<(), Error> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn get_bundle(&self, name: &str) -> Option<&BundleInfo> {
        self.bundles.get(name)
    }

    /// Searches for asset bundles in the specified directory and generates validation data for each one.
    async fn get_bundle_info(
        asset_root: &str,
    ) -> Result<(u64, u64, HashMap<String, BundleInfo>), Error> {
        let bundle_names = get_bundle_names_from_asset_root(asset_root)?;
        info!("Found {} bundles", bundle_names.len());
        info!("Processing...");

        let bundles: Arc<Mutex<HashMap<String, BundleInfo>>> = Arc::new(Mutex::new(HashMap::new()));
        let mut tasks: Vec<JoinHandle<Result<(), String>>> = Vec::with_capacity(bundle_names.len());
        for bundle_name in bundle_names {
            let root = asset_root.to_string();
            let bundles = Arc::clone(&bundles);
            tasks.push(tokio::spawn(async move {
                let _permit = if let Some(permits) = ITEM_PERMITS.get() {
                    Some(permits.acquire().await.unwrap())
                } else {
                    None
                };

                let bundle_info = BundleInfo::build(&root, &bundle_name)
                    .await
                    .map_err(|e| e.to_string())?;
                debug!("Processed {}", bundle_name);
                bundles.lock().unwrap().insert(bundle_name, bundle_info);
                Ok(())
            }));
        }

        for task in tasks {
            if let Err(e) = task.await? {
                return Err(e.into());
            }
        }
        info!("Done processing");

        let bundles = Arc::try_unwrap(bundles).unwrap().into_inner().unwrap();
        let total_compressed_size = bundles.values().map(|b| b.compressed_info.size).sum();
        let total_uncompressed_size = bundles.values().map(|b| b.get_uncompressed_size()).sum();
        info!("{} bytes compressed", total_compressed_size);
        info!("{} bytes uncompressed", total_uncompressed_size);
        Ok((total_compressed_size, total_uncompressed_size, bundles))
    }

    /// Validates the compressed asset bundles against the metadata. Returns a list of corrupted bundles.
    pub async fn validate_compressed(
        &self,
        path: &str,
        callback: Option<ProgressCallback>,
    ) -> Result<Vec<String>, Error> {
        self.validate_compressed_internal(path, false, false, callback)
            .await
    }

    /// Validates the compressed asset bundles against the metadata. Stops on the first failure.
    /// Returns the name of the first corrupted bundle.
    pub async fn validate_compressed_stop_on_first_fail(
        &self,
        path: &str,
        callback: Option<ProgressCallback>,
    ) -> Result<Option<String>, Error> {
        let corrupted = self
            .validate_compressed_internal(path, false, true, callback)
            .await?;
        Ok(corrupted.first().cloned())
    }

    /// Validates the compressed asset bundles against the metadata. Returns a list of corrupted bundles.
    /// If `download_failed_bundles` is true, corrupted bundles will be re-downloaded.
    /// If `stop_on_first_fail` is true, the function will return as soon as it encounters a corrupted bundle.
    async fn validate_compressed_internal(
        &self,
        path: &str,
        download_failed_bundles: bool,
        stop_on_first_fail: bool,
        callback: Option<ProgressCallback>,
    ) -> Result<Vec<String>, Error> {
        info!(
            "Validating compressed asset bundles for {} ({})...",
            self.uuid, path
        );

        let get_path =
            |name: &str| -> String { PathBuf::from(path).join(name).to_str().unwrap().to_string() };
        let mut corrupted_bundles = Vec::with_capacity(self.bundles.len() + 1);

        if let Some(main_file_info) = self.main_file_info.clone() {
            info!("Checking main file");
            let main_bundle_info: BundleInfo = main_file_info.into();
            let main_file_path = get_path("main.unity3d");
            let main_file_url = match download_failed_bundles {
                false => None,
                true => Some(format!("{}/main.unity3d", self.get_asset_url())),
            };
            if main_bundle_info
                .validate_compressed(
                    &main_file_path,
                    Some(self.uuid),
                    main_file_url.as_deref(),
                    callback.clone(),
                )
                .await
                .is_err()
            {
                if stop_on_first_fail {
                    info!("Main file corrupted");
                    return Ok(vec!["main.unity3d".to_string()]);
                } else {
                    corrupted_bundles.push("main.unity3d".to_string());
                }
            }
        }

        info!("Checking asset bundles");
        let bundles = self.bundles.clone();
        let repair_count = Arc::new(AtomicU64::new(0));
        let corrupted = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::with_capacity(bundles.len());
        for (bundle_name, bundle_info) in bundles {
            let cb = callback.clone();
            let file_path = get_path(&bundle_name);
            let repair_count = Arc::clone(&repair_count);
            let corrupted = Arc::clone(&corrupted);
            let url = match download_failed_bundles {
                false => None,
                true => Some(format!("{}/{}", self.get_asset_url(), bundle_name)),
            };
            let uuid = self.uuid;
            tasks.push(tokio::spawn(async move {
                let _permit = if let Some(permits) = crate::ITEM_PERMITS.get() {
                    Some(permits.acquire().await.unwrap())
                } else {
                    None
                };

                match bundle_info
                    .validate_compressed(&file_path, Some(uuid), url.as_deref(), cb)
                    .await
                {
                    Ok(true) => {
                        info!("{} repaired", bundle_name);
                        corrupted.lock().unwrap().push(bundle_name);
                        repair_count.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(false) => {
                        debug!("{} validated", bundle_name);
                    }
                    Err(e) => {
                        warn!("{} failed validation: {}", bundle_name, e);
                        corrupted.lock().unwrap().push(bundle_name);
                    }
                }
            }));
        }

        for task in tasks {
            task.await?;
            if stop_on_first_fail {
                let corrupted = corrupted.lock().unwrap();
                if let Some(bundle) = corrupted.first() {
                    info!(
                        "Validation complete; at least {} corrupted bundles",
                        corrupted.len()
                    );
                    return Ok(vec![bundle.clone()]);
                }
            }
        }

        let repair_count = repair_count.load(Ordering::SeqCst);
        corrupted_bundles.extend(Arc::try_unwrap(corrupted).unwrap().into_inner().unwrap());
        info!(
            "Validation complete; {}/{} missing or corrupted bundles repaired",
            repair_count,
            corrupted_bundles.len()
        );
        Ok(corrupted_bundles)
    }

    /// Validates the uncompressed asset bundles against the metadata. Returns a list of corrupted files.
    pub async fn validate_uncompressed(
        &self,
        path: &str,
        callback: Option<ProgressCallback>,
    ) -> Result<Vec<String>, Error> {
        self.validate_uncompressed_internal(path, false, callback)
            .await
    }

    /// Validates the uncompressed asset bundles against the metadata. Stops on the first failure.
    /// Returns the name of the first corrupted file.
    pub async fn validate_uncompressed_stop_on_first_fail(
        &self,
        path: &str,
        callback: Option<ProgressCallback>,
    ) -> Result<Option<String>, Error> {
        let corrupted = self
            .validate_uncompressed_internal(path, true, callback)
            .await?;
        Ok(corrupted.first().cloned())
    }

    /// Validates the uncompressed asset bundles against the metadata. Returns a list of corrupted files.
    /// If `stop_on_first_fail` is true, the function will return as soon as it encounters a corrupted file.
    async fn validate_uncompressed_internal(
        &self,
        path: &str,
        stop_on_first_fail: bool,
        callback: Option<ProgressCallback>,
    ) -> Result<Vec<String>, Error> {
        info!(
            "Validating uncompressed asset bundles for {} ({})...",
            self.uuid, path
        );
        let bundles = self.bundles.clone();
        let corrupted = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::with_capacity(bundles.len());
        for (bundle_name, bundle_info) in bundles {
            let cb = callback.clone();
            let corrupted = Arc::clone(&corrupted);
            let bundle_name_url_encoded = util::url_encode(&bundle_name);
            let folder_path = PathBuf::from(path).join(&bundle_name_url_encoded);
            let uuid = self.uuid;
            tasks.push(tokio::spawn(async move {
                let _permit = if let Some(permits) = crate::ITEM_PERMITS.get() {
                    Some(permits.acquire().await.unwrap())
                } else {
                    None
                };

                match bundle_info.validate_uncompressed(
                    folder_path.to_str().unwrap(),
                    Some(uuid),
                    cb,
                ) {
                    Ok(corrupted_files) => {
                        if !corrupted_files.is_empty() {
                            for (file_name, e) in &corrupted_files {
                                warn!("{} failed validation: {}", file_name, e);
                            }
                            corrupted.lock().unwrap().extend(
                                corrupted_files.into_iter().map(|(file_name, _)| file_name),
                            );
                        }
                    }
                    Err(e) => {
                        warn!("{} failed validation: {}", bundle_name, e);
                        corrupted.lock().unwrap().push(bundle_name);
                    }
                }
            }));
        }

        for task in tasks {
            task.await?;
            if stop_on_first_fail {
                let corrupted = corrupted.lock().unwrap();
                if let Some(file) = corrupted.first() {
                    info!(
                        "Validation complete; at least {} corrupted files",
                        corrupted.len()
                    );
                    return Ok(vec![file.clone()]);
                }
            }
        }

        let corrupted = Arc::try_unwrap(corrupted).unwrap().into_inner().unwrap();
        info!("Validation complete; {} corrupted files", corrupted.len());
        Ok(corrupted)
    }

    /// Downloads all compressed asset bundles and the main file for this build to the specified path.
    pub async fn download_compressed(
        &self,
        path: &str,
        callback: Option<ProgressCallback>,
    ) -> Result<(), Error> {
        info!("Downloading build {} to {}", self.uuid, path,);
        let _ = std::fs::remove_dir_all(path);
        std::fs::create_dir_all(path)?;
        self.repair(path, callback).await?;
        info!("Download complete");
        Ok(())
    }

    /// Repairs the build by re-downloading corrupted asset bundles.
    pub async fn repair(
        &self,
        path: &str,
        callback: Option<ProgressCallback>,
    ) -> Result<Vec<String>, Error> {
        if !std::fs::exists(path).unwrap_or(false) {
            return Err(format!("Path does not exist: {}", path).into());
        }
        let uuid = self.uuid;
        info!("Repairing build {} at {}", uuid, path);
        let corrupted = self
            .validate_compressed_internal(path, true, false, callback)
            .await?;
        info!("Repair complete");
        Ok(corrupted)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct BundleInfo {
    compressed_info: FileInfo,
    uncompressed_info: HashMap<String, FileInfo>,
}
impl From<FileInfo> for BundleInfo {
    fn from(compressed_info: FileInfo) -> Self {
        Self {
            compressed_info,
            uncompressed_info: HashMap::new(),
        }
    }
}
impl BundleInfo {
    async fn build(asset_root: &str, bundle_name: &str) -> Result<Self, Error> {
        let file_path = format!("{}/{}", asset_root, bundle_name);

        let compressed_info = FileInfo::build(&file_path).await?;
        let bundle = AssetBundle::from_file(&file_path)?;
        if bundle.get_file_size() != compressed_info.size {
            warn!(
                "File size mismatch: {} (header) vs {} (actual) for {}",
                bundle.get_file_size(),
                compressed_info.size,
                bundle_name
            );
        }

        #[cfg(feature = "lzma")]
        let uncompressed_info = bundle.get_uncompressed_info().await?;

        #[cfg(not(feature = "lzma"))]
        let uncompressed_info = HashMap::new();

        Ok(Self {
            compressed_info,
            uncompressed_info,
        })
    }

    fn get_uncompressed_size(&self) -> u64 {
        self.uncompressed_info.values().map(|info| info.size).sum()
    }

    /// Validates the compressed asset bundle against the metadata.
    /// If the file is valid, the function returns `Ok(false)`.
    /// If the file fails validation, it will be re-downloaded up to `MAX_DOWNLOAD_ATTEMPTS` times.
    /// If the file was successfully re-downloaded, the function returns `Ok(true)`.
    /// If the file is still corrupted after the maximum number of attempts, an error will be returned.
    pub async fn validate_compressed(
        &self,
        file_path: &str,
        version_uuid: Option<Uuid>,
        download_url: Option<&str>,
        callback: Option<ProgressCallback>,
    ) -> Result<bool, Error> {
        const MAX_DOWNLOAD_ATTEMPTS: usize = 5;
        let file_name = util::get_file_name_without_parent(file_path);
        let mut file_info = FileInfo::build_file(file_path);
        let mut attempts = 0;
        while let Err(fail_reason) = {
            if let Some(ref cb) = callback {
                let uuid = version_uuid.unwrap_or_default();
                cb(&uuid, file_name, ItemProgress::Validating);
            }
            file_info.validate(&self.compressed_info)
        } {
            warn!("{} invalid", file_name);
            let Some(url) = download_url else {
                if let Some(ref cb) = callback {
                    let uuid = version_uuid.unwrap_or_default();
                    cb(
                        &uuid,
                        file_name,
                        ItemProgress::Failed {
                            item_size: self.compressed_info.size,
                            reason: fail_reason.clone(),
                        },
                    );
                }
                return Err(fail_reason.clone().into());
            };

            if attempts >= MAX_DOWNLOAD_ATTEMPTS {
                if let Some(ref cb) = callback {
                    let uuid = version_uuid.unwrap_or_default();
                    cb(
                        &uuid,
                        file_name,
                        ItemProgress::Failed {
                            item_size: self.compressed_info.size,
                            reason: fail_reason.clone(),
                        },
                    );
                }
                return Err(format!(
                    "Failed to download {} after {} attempts: {}",
                    file_path, attempts, fail_reason
                )
                .into());
            }

            if let Err(e) =
                util::download_to_file(version_uuid, url, file_path, callback.clone()).await
            {
                warn!("Failed to download {}: {}", file_path, e);
            } else {
                file_info = FileInfo::build_file(file_path);
            }
            attempts += 1;
        }

        if let Some(ref cb) = callback {
            let uuid = version_uuid.unwrap_or_default();
            cb(
                &uuid,
                file_name,
                ItemProgress::Passed {
                    item_size: self.compressed_info.size,
                },
            );
        }
        Ok(attempts > 0)
    }

    pub fn validate_uncompressed(
        &self,
        folder_path: &str,
        version_uuid: Option<Uuid>,
        callback: Option<ProgressCallback>,
    ) -> Result<Vec<(String, FailReason)>, Error> {
        let uuid = version_uuid.unwrap_or_default();
        let folder_path_leaf = util::get_file_name_without_parent(folder_path);
        let mut corrupted = Vec::new();
        for (file_name, file_info_good) in &self.uncompressed_info {
            let file_path = PathBuf::from(folder_path).join(file_name);
            let file_info = FileInfo::build_file(file_path.to_str().unwrap());
            let file_id = format!("{}/{}", folder_path_leaf, file_name);

            if let Some(ref cb) = callback {
                cb(&uuid, &file_id, ItemProgress::Validating);
            }

            let mut result = ItemProgress::Passed {
                item_size: file_info_good.size,
            };
            if let Err(fail_reason) = file_info.validate(file_info_good) {
                warn!("{} invalid: {}", file_id, fail_reason);
                corrupted.push((file_id.clone(), fail_reason.clone()));
                result = ItemProgress::Failed {
                    item_size: file_info_good.size,
                    reason: fail_reason,
                };
            }

            if let Some(ref cb) = callback {
                cb(&uuid, &file_id, result);
            }
        }
        Ok(corrupted)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FileInfo {
    hash: String,
    size: u64,
}
impl FileInfo {
    async fn build(uri: &str) -> Result<Self, Error> {
        if uri.starts_with("http") {
            Self::build_http(uri).await
        } else {
            Ok(Self::build_file(uri))
        }
    }

    async fn build_http(url: &str) -> Result<Self, Error> {
        info!("Fetching {}", url);
        let temp_file = TempFile::download(url).await?;
        Ok(Self::build_file(temp_file.path()))
    }

    fn build_file(file_path: &str) -> Self {
        let build_file_internal = || -> Result<Self, Error> {
            let hash = util::get_file_hash(file_path)?;
            let size = std::fs::metadata(file_path)?.len();
            Ok(Self { hash, size })
        };
        // if we can't access the file, assume it's corrupt
        build_file_internal().unwrap_or_default()
    }

    #[cfg(feature = "lzma")]
    fn build_buffer(buffer: &[u8]) -> Self {
        let hash = util::get_buffer_hash(buffer);
        let size = buffer.len() as u64;
        Self { hash, size }
    }

    fn validate(&self, good: &Self) -> Result<(), FailReason> {
        if self.size == 0 {
            return Err(FailReason::Missing);
        }

        if self.size != good.size {
            return Err(FailReason::BadSize {
                expected: good.size,
                actual: self.size,
            });
        }

        if self.hash != good.hash {
            return Err(FailReason::BadHash {
                expected: good.hash.clone(),
                actual: self.hash.clone(),
            });
        }

        Ok(())
    }
}

fn get_bundle_names_from_asset_root(asset_root: &str) -> Result<Vec<String>, Error> {
    let filtered = util::list_filenames_in_directory(asset_root)?
        .iter()
        .filter_map(|filename| {
            if filename.eq_ignore_ascii_case("main.unity3d") {
                None
            } else {
                let extension = util::get_file_extension(filename)?;
                if extension.eq_ignore_ascii_case("unity3d")
                    || extension.eq_ignore_ascii_case("resourceFile")
                {
                    Some(filename.to_string())
                } else {
                    None
                }
            }
        })
        .collect();
    Ok(filtered)
}
