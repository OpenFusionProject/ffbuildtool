use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use bundle::AssetBundle;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use util::TempFile;
use uuid::Uuid;

use log::*;

type Error = Box<dyn std::error::Error>;

pub mod bundle;
pub mod util;

#[cfg(test)]
mod tests;

/// Contains all the info comprising a FusionFall build.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Version {
    uuid: Uuid,
    description: Option<String>,
    parent_uuid: Option<Uuid>,
    /// The main file may be located outside of the asset root.
    main_file_url: String,
    main_file_info: FileInfo,
    asset_info: AssetInfo,
}
impl Version {
    /// Generates `Version` metadata given a local build root (compressed asset bundles).
    pub async fn build(
        main_path: &str,
        main_url: &str,
        asset_root: &str,
        asset_url: &str,
        description: Option<&str>,
        parent: Option<Uuid>,
    ) -> Result<Self, Error> {
        let main_file_info = FileInfo::build(main_path).await?;
        let asset_info = AssetInfo::build(asset_root, asset_url).await?;
        Ok(Self {
            uuid: Uuid::new_v4(),
            description: description.map(|s| s.to_string()),
            parent_uuid: parent,
            main_file_url: main_url.to_string(),
            main_file_info,
            asset_info,
        })
    }

    pub fn get_uuid(&self) -> Uuid {
        self.uuid
    }

    pub fn set_asset_url(&mut self, asset_url: &str) {
        self.asset_info.asset_url = asset_url.to_string();
    }

    pub fn from_manifest(path: &str) -> Result<Self, Error> {
        let json = std::fs::read_to_string(path)?;
        let version: Self = serde_json::from_str(&json)?;
        Ok(version)
    }

    pub async fn from_manifest_url(url: &str) -> Result<Self, Error> {
        let manifest = TempFile::download(url).await?;
        let version = Self::from_manifest(manifest.path())?;
        Ok(version)
    }

    /// Exports the `Version` metadata to a JSON file to be served from an API server.
    pub fn export_manifest(&self, path: &str) -> Result<(), Error> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn get_bundle(&self, name: &str) -> Option<&BundleInfo> {
        self.asset_info.bundles.get(name)
    }

    /// Validates the compressed asset bundles against the metadata. Returns a list of corrupted bundles.
    pub async fn validate_compressed(&self, path: &str) -> Result<Vec<String>, Error> {
        info!(
            "Validating compressed asset bundles for {} ({})...",
            self.uuid, path
        );
        let bundles = self.asset_info.bundles.clone();
        let corrupted = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::with_capacity(bundles.len());
        for (bundle_name, bundle_info) in bundles {
            let file_path = PathBuf::from(path)
                .join(&bundle_name)
                .to_str()
                .unwrap()
                .to_string();
            let corrupted = Arc::clone(&corrupted);
            tasks.push(tokio::spawn(async move {
                if let Err(e) = bundle_info.validate_compressed(&file_path) {
                    warn!("{} failed validation: {}", bundle_name, e);
                    corrupted.lock().unwrap().push(bundle_name);
                }
            }));
        }

        for task in tasks {
            task.await?;
        }

        let corrupted = Arc::try_unwrap(corrupted).unwrap().into_inner().unwrap();
        info!("Validation complete; {} corrupted bundles", corrupted.len());
        Ok(corrupted)
    }

    pub async fn validate_uncompressed(&self, path: &str) -> Result<Vec<String>, Error> {
        info!(
            "Validating uncompressed asset bundles for {} ({})...",
            self.uuid, path
        );
        let bundles = self.asset_info.bundles.clone();
        let corrupted = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::with_capacity(bundles.len());
        for (bundle_name, bundle_info) in bundles {
            let corrupted = Arc::clone(&corrupted);
            let bundle_name_url_encoded = util::url_encode(&bundle_name);
            let folder_path = PathBuf::from(path).join(&bundle_name_url_encoded);
            tasks.push(tokio::spawn(async move {
                match bundle_info.validate_uncompressed(folder_path.to_str().unwrap()) {
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
        }

        let corrupted = Arc::try_unwrap(corrupted).unwrap().into_inner().unwrap();
        info!("Validation complete; {} corrupted files", corrupted.len());
        Ok(corrupted)
    }

    pub async fn download_compressed(&self, path: &str) -> Result<(), Error> {
        info!("Downloading build {} to {}", self.uuid, path);
        std::fs::create_dir_all(path)?;
        let path = PathBuf::from(path);

        // download and validate main file
        let main_file_path = path.join("main.unity3d").to_str().unwrap().to_string();
        util::download_to_file(&self.main_file_url, &main_file_path).await?;
        let main_file_info = FileInfo::build_file(&main_file_path).unwrap();
        main_file_info.validate(&self.main_file_info)?;

        let bundle_names = self.asset_info.bundles.keys().clone();
        let mut tasks: Vec<tokio::task::JoinHandle<Result<(), String>>> =
            Vec::with_capacity(bundle_names.len());
        info!("Downloading assets...");
        for bundle_name in bundle_names {
            let file_url = format!("{}/{}", self.asset_info.asset_url, bundle_name);
            let file_path = path.join(bundle_name).to_str().unwrap().to_string();
            tasks.push(tokio::spawn(async move {
                util::download_to_file(&file_url, &file_path)
                    .await
                    .map_err(|e| format!("Failed to download {}: {}", file_url, e))?;
                debug!("Downloaded {}", file_url);
                Ok(())
            }));
        }

        for task in tasks {
            if let Err(e) = task.await? {
                return Err(e.into());
            }
        }
        info!("Download complete");

        self.validate_compressed(path.to_str().unwrap()).await?;

        Ok(())
    }

    pub async fn repair(&self, path: &str) -> Result<Vec<String>, Error> {
        info!("Repairing build {} at {}", self.uuid, path);
        let corrupted = self.validate_compressed(path).await?;
        info!("{} corrupted bundles: {:?}", corrupted.len(), corrupted);

        let mut tasks: Vec<JoinHandle<Result<(), String>>> = Vec::with_capacity(corrupted.len());
        for bundle_name in corrupted.clone() {
            let url = format!("{}/{}", self.asset_info.asset_url, bundle_name);
            let bundle_info = self.get_bundle(&bundle_name).unwrap().clone();
            let file_path = PathBuf::from(path)
                .join(&bundle_name)
                .to_str()
                .unwrap()
                .to_string();
            tasks.push(tokio::spawn(async move {
                if std::fs::exists(&file_path).is_ok_and(|exists| exists) {
                    std::fs::remove_file(&file_path).map_err(|e| e.to_string())?;
                }
                util::download_to_file(&url, &file_path)
                    .await
                    .map_err(|e| e.to_string())?;
                bundle_info
                    .validate_compressed(&file_path)
                    .map_err(|e| e.to_string())?;
                debug!("Repaired {}", bundle_name);
                Ok(())
            }));
        }

        for task in tasks {
            if let Err(e) = task.await? {
                return Err(e.into());
            }
        }

        info!("Repair complete");
        Ok(corrupted)
    }
}

/// Contains the info for each asset bundle in the build.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct AssetInfo {
    asset_url: String,
    total_compressed_size: u64,
    total_uncompressed_size: u64,
    bundles: HashMap<String, BundleInfo>,
}
impl AssetInfo {
    async fn build(asset_root: &str, asset_url: &str) -> Result<Self, Error> {
        let (total_compressed_size, total_uncompressed_size, bundles) =
            Self::get_bundle_info(asset_root).await?;
        Ok(Self {
            asset_url: asset_url.to_string(),
            total_compressed_size,
            total_uncompressed_size,
            bundles,
        })
    }

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
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct BundleInfo {
    compressed_info: FileInfo,
    uncompressed_info: HashMap<String, FileInfo>,
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

    pub fn validate_compressed(&self, file_path: &str) -> Result<(), Error> {
        let file_info = FileInfo::build_file(file_path)?;
        file_info.validate(&self.compressed_info)?;
        Ok(())
    }

    pub fn validate_uncompressed(&self, folder_path: &str) -> Result<Vec<(String, String)>, Error> {
        let folder_path_leaf = util::get_file_name_without_parent(folder_path);
        let mut corrupted = Vec::new();
        for (file_name, file_info_good) in &self.uncompressed_info {
            let file_path = PathBuf::from(folder_path).join(file_name);
            let file_info = FileInfo::build_file(file_path.to_str().unwrap())?;
            if let Err(e) = file_info.validate(file_info_good) {
                let file_id = format!("{}/{}", folder_path_leaf, file_name);
                corrupted.push((file_id, e));
            }
        }
        Ok(corrupted)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FileInfo {
    hash: String,
    size: u64,
}
impl FileInfo {
    async fn build(uri: &str) -> Result<Self, Error> {
        if uri.starts_with("http") {
            Self::build_http(uri).await
        } else {
            Self::build_file(uri)
        }
    }

    async fn build_http(url: &str) -> Result<Self, Error> {
        info!("Fetching {}", url);
        let temp_file = TempFile::download(url).await?;
        Self::build_file(temp_file.path())
    }

    fn build_file(file_path: &str) -> Result<Self, Error> {
        let build_file_internal = || -> Result<Self, Error> {
            let hash = util::get_file_hash(file_path)?;
            let size = std::fs::metadata(file_path)?.len();
            Ok(Self { hash, size })
        };
        build_file_internal().map_err(|_| format!("File not found: {}", file_path).into())
    }

    #[cfg(feature = "lzma")]
    fn build_buffer(buffer: &[u8]) -> Self {
        let hash = util::get_buffer_hash(buffer);
        let size = buffer.len() as u64;
        Self { hash, size }
    }

    fn validate(&self, good: &Self) -> Result<(), String> {
        if self.hash != good.hash {
            return Err(format!(
                "Bad hash: {} (disk) vs {} (manifest)",
                self.hash, good.hash
            ));
        }

        if self.size != good.size {
            return Err(format!(
                "Bad size: {} (disk) vs {} (manifest)",
                self.size, good.size
            ));
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
