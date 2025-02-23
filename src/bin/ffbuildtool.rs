use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use clap::{Args, Parser, Subcommand};

use ffbuildtool::{ItemProgress, Version};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    GenManifest(GenManifestArgs),
    DownloadBuild(DownloadBuildArgs),
    RepairBuild(RepairBuildArgs),
    ValidateBuild(ValidateBuildArgs),
    #[cfg(feature = "lzma")]
    ReadBundle(ReadBundleArgs),
    #[cfg(feature = "lzma")]
    ExtractBundle(ExtractBundleArgs),
    #[cfg(feature = "lzma")]
    PackBundle(PackBundleArgs),
}

#[derive(Args, Debug)]
struct GenManifestArgs {
    /// Path to the directory containing all the compressed asset bundles in the build
    #[clap(short = 'b', long)]
    build_path: String,

    /// URL that will point to the directory containing all the compressed asset bundles in the build
    #[clap(short = 'u', long)]
    asset_url: String,

    /// Name of the build
    #[clap(short = 'n', long)]
    name: Option<String>,

    /// Description of the build
    #[clap(short = 'd', long)]
    description: Option<String>,

    /// UUID of the parent build
    #[clap(short = 'p', long)]
    parent: Option<String>,

    /// Path to the output manifest file
    #[clap(short = 'o', long)]
    output_path: String,

    /// Whether the version should be marked as hidden
    #[clap(long)]
    hidden: bool,
}

#[derive(Args, Debug)]
struct DownloadBuildArgs {
    /// Path to the manifest file
    #[clap(short = 'm', long)]
    manifest_path: String,

    /// Path to the directory where all the compressed asset bundles in the build, along with the main file, will be downloaded
    #[clap(short = 'o', long)]
    output_path: String,
}

#[derive(Args, Debug)]
struct RepairBuildArgs {
    /// Path to the manifest file
    #[clap(short = 'm', long)]
    manifest_path: String,

    /// Path to the directory containing the compressed asset bundles in the build
    #[clap(short = 'p', long)]
    build_path: String,
}

#[derive(Args, Debug)]
struct ValidateBuildArgs {
    /// Path to the manifest file
    #[clap(short = 'm', long)]
    manifest_path: String,

    /// Path to the directory containing the asset bundles in the build
    #[clap(short = 'p', long)]
    build_path: String,

    /// Flag indicating that the bundles are uncompressed
    #[clap(short = 'u', long)]
    uncompressed: bool,
}

#[cfg(feature = "lzma")]
#[derive(Args, Debug)]
struct ReadBundleArgs {
    /// Path to the compressed asset bundle
    #[clap(short = 'i', long)]
    input_bundle: String,

    /// Whether to calculate the hashes of each file in the bundle
    #[clap(short = 'c', long, action)]
    calculate_hashes: bool,
}

#[cfg(feature = "lzma")]
#[derive(Args, Debug)]
struct ExtractBundleArgs {
    /// Path to the compressed asset bundle
    #[clap(short = 'i', long)]
    input_bundle: String,

    /// Path to the output directory. If not specified, will be extracted to a directory named after the bundle.
    #[clap(short = 'o', long)]
    output_dir: Option<String>,
}

#[cfg(feature = "lzma")]
#[derive(Args, Debug)]
struct PackBundleArgs {
    /// Path to the input directory
    #[clap(short = 'i', long)]
    input_dir: String,

    /// Path to the output bundle
    #[clap(short = 'o', long)]
    output_bundle: String,

    /// Compression level to use
    #[clap(short = 'l', long, default_value = "4")]
    compression_level: u32,
}

#[derive(Debug, PartialEq, Eq)]
enum ItemState {
    Downloading,
    Validating,
}

struct ProgressManager {
    multi: MultiProgress,
    bars: Mutex<HashMap<String, (ProgressBar, ItemState)>>,
    max_bars: usize,
    styles: Vec<ProgressStyle>,
}
impl ProgressManager {
    fn new() -> Self {
        Self {
            multi: MultiProgress::new(),
            bars: Mutex::new(HashMap::new()),
            max_bars: 10,
            styles: vec![
                ProgressStyle::default_bar()
                    .template("[{bar:40}] {bytes} / {total_bytes} ({eta}) {wide_msg:>}")
                    .unwrap()
                    .progress_chars("=> "),
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} Validating {wide_msg:>}")
                    .unwrap(),
            ],
        }
    }

    fn update_item(&self, name: &str, progress: ItemProgress) {
        match progress {
            ItemProgress::Downloading {
                bytes_downloaded,
                total_bytes,
            } => {
                self.update_item_downloading(name, bytes_downloaded, total_bytes);
            }
            ItemProgress::Validating => {
                self.update_item_validating(name);
            }
            ItemProgress::Passed { .. } | ItemProgress::Failed { .. } => {
                self.finish_item(name);
            }
        }
    }

    fn finish_item(&self, name: &str) {
        let mut bars = self.bars.lock().unwrap();
        if let Some((pb, _)) = bars.remove(name) {
            pb.finish_and_clear();
        }
    }

    fn update_item_validating(&self, name: &str) {
        let mut bars = self.bars.lock().unwrap();
        if let Some((pb, st)) = bars.get_mut(name) {
            if *st != ItemState::Validating {
                pb.set_style(self.styles[1].clone());
                *st = ItemState::Validating;
            }
        } else if bars.len() < self.max_bars {
            let pb = self.multi.add(ProgressBar::new(0));
            pb.set_style(self.styles[1].clone());
            pb.set_message(name.to_string());
            pb.enable_steady_tick(Duration::from_millis(100));
            bars.insert(name.to_string(), (pb, ItemState::Validating));
        };
    }

    fn update_item_downloading(&self, name: &str, current: u64, total: u64) {
        let mut bars = self.bars.lock().unwrap();
        if let Some((pb, st)) = bars.get_mut(name) {
            if *st != ItemState::Downloading {
                pb.disable_steady_tick();
                pb.set_style(self.styles[0].clone());
                pb.set_length(total);
                *st = ItemState::Downloading;
            }
            if pb.length().unwrap_or(0) != total {
                pb.set_length(total);
            }
            pb.set_position(current);
        } else if bars.len() < self.max_bars {
            let pb = self.multi.add(ProgressBar::new(total));
            pb.set_style(self.styles[0].clone());
            pb.set_message(name.to_string());
            pb.set_position(current);
            bars.insert(name.to_string(), (pb, ItemState::Downloading));
        };
    }
}

async fn parse_manifest(path: &str) -> Result<Version, String> {
    Version::from_manifest(path)
        .await
        .map_err(|e| format!("Couldn't parse manifest: {}", e))
}

static PROGRESS: OnceLock<ProgressManager> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<(), String> {
    PROGRESS
        .set(ProgressManager::new())
        .unwrap_or_else(|_| panic!());
    let args = Cli::parse();
    match args.command {
        Commands::GenManifest(args) => generate_manifest(args).await,
        Commands::DownloadBuild(args) => download_build(args).await,
        Commands::RepairBuild(args) => repair_build(args).await,
        Commands::ValidateBuild(args) => validate_build(args).await,
        #[cfg(feature = "lzma")]
        Commands::ReadBundle(args) => read_bundle(args).await,
        #[cfg(feature = "lzma")]
        Commands::ExtractBundle(args) => extract_bundle(args).await,
        #[cfg(feature = "lzma")]
        Commands::PackBundle(args) => pack_bundle(args).await,
    }
}

async fn generate_manifest(args: GenManifestArgs) -> Result<(), String> {
    println!(
        "Generating manifest for build at {} with asset URL {}",
        args.build_path, args.asset_url
    );
    let parent_uuid: Option<Uuid> = if let Some(p) = args.parent {
        Some(Uuid::parse_str(p.as_str()).map_err(|_| "Invalid parent UUID".to_string())?)
    } else {
        None
    };

    let mut version = Version::build(
        &args.build_path,
        &args.asset_url,
        args.name.as_deref(),
        args.description.as_deref(),
        parent_uuid,
    )
    .await
    .map_err(|e| format!("Couldn't generate bundle info: {}", e))?;

    if args.hidden {
        version.set_hidden(true);
    }

    println!("Build UUID: {}", version.get_uuid());

    version
        .export_manifest(&args.output_path)
        .map_err(|e| format!("Couldn't export manifest: {}", e))?;
    println!("Manifest exported to {}", args.output_path);
    Ok(())
}

async fn download_build(args: DownloadBuildArgs) -> Result<(), String> {
    let version = parse_manifest(&args.manifest_path).await?;
    println!(
        "Downloading build {} to {}",
        version.get_uuid(),
        args.output_path
    );

    let cb = |_uuid: &Uuid, name: &str, progress: ItemProgress| {
        PROGRESS.get().unwrap().update_item(name, progress);
    };

    version
        .download_compressed(&args.output_path, Some(Arc::new(cb)))
        .await
        .map_err(|e| format!("Couldn't download build: {}", e))?;
    println!("Download complete");
    Ok(())
}

async fn repair_build(args: RepairBuildArgs) -> Result<(), String> {
    let version = parse_manifest(&args.manifest_path).await?;
    println!(
        "Repairing build {} at {}",
        version.get_uuid(),
        args.build_path
    );

    let cb = |_uuid: &Uuid, name: &str, progress: ItemProgress| {
        PROGRESS.get().unwrap().update_item(name, progress);
    };

    let corrupted = version
        .repair(&args.build_path, Some(Arc::new(cb)))
        .await
        .map_err(|e| format!("Couldn't repair build: {}", e))?;
    if corrupted.is_empty() {
        println!("No corrupted files found");
    } else {
        println!("{} corrupted files repaired:", corrupted.len());
        for file in corrupted {
            println!("\t{}", file);
        }
    }
    Ok(())
}

async fn validate_build(args: ValidateBuildArgs) -> Result<(), String> {
    let version = parse_manifest(&args.manifest_path).await?;
    println!(
        "Validating build {} at {}",
        version.get_uuid(),
        args.build_path
    );

    let cb = |_uuid: &Uuid, name: &str, progress: ItemProgress| {
        PROGRESS.get().unwrap().update_item(name, progress);
    };

    let corrupted = if args.uncompressed {
        version
            .validate_uncompressed(&args.build_path, None)
            .await
            .map_err(|e| format!("Couldn't validate uncompressed files: {}", e))?
    } else {
        version
            .validate_compressed(&args.build_path, Some(Arc::new(cb)))
            .await
            .map_err(|e| format!("Couldn't validate compressed files: {}", e))?
    };

    if corrupted.is_empty() {
        println!("No corrupted files found");
    } else {
        println!("{} corrupted files found:", corrupted.len());
        for file in corrupted {
            println!("\t{}", file);
        }
    }
    Ok(())
}

#[cfg(feature = "lzma")]
async fn read_bundle(args: ReadBundleArgs) -> Result<(), String> {
    use std::time::Instant;

    use ffbuildtool::bundle::AssetBundle;

    let start = Instant::now();
    let (header, mut bundle) = AssetBundle::from_file(&args.input_bundle)?;
    println!("Bundle read in {}ms", start.elapsed().as_millis());

    if args.calculate_hashes {
        let start = Instant::now();
        bundle.recalculate_all_hashes();
        println!("Hashes calculated in {}ms", start.elapsed().as_millis());
    }

    println!(
        "------------------------\n{}\n------------------------\n{}",
        header, bundle
    );
    Ok(())
}

#[cfg(feature = "lzma")]
async fn extract_bundle(args: ExtractBundleArgs) -> Result<(), String> {
    use std::{path::PathBuf, time::Instant};

    use ffbuildtool::{bundle::AssetBundle, util};

    let start = Instant::now();
    let (header, bundle) = AssetBundle::from_file(&args.input_bundle)?;
    println!("Bundle read in {}ms", start.elapsed().as_millis());
    println!(
        "------------------------\n{}\n------------------------\n{}",
        header, bundle
    );

    let output_dir = args.output_dir.unwrap_or({
        let bundle_name = util::get_file_name_without_parent(&args.input_bundle);
        let bundle_name_url_encoded = util::url_encode(bundle_name);
        let bundle_path = PathBuf::from(&args.input_bundle);
        bundle_path
            .parent()
            .unwrap_or(&bundle_path)
            .join(bundle_name_url_encoded)
            .to_string_lossy()
            .to_string()
    });
    println!("Extracting bundle {} to {}", args.input_bundle, output_dir);

    let start = Instant::now();
    bundle.extract_files(&output_dir)?;
    println!("Bundle extracted in {}ms", start.elapsed().as_millis());

    Ok(())
}

#[cfg(feature = "lzma")]
async fn pack_bundle(args: PackBundleArgs) -> Result<(), String> {
    use std::{sync::LazyLock, time::Instant};

    use ffbuildtool::bundle::AssetBundle;

    fn cb(level_idx: usize, file: usize, total_files: usize, current_file_name: String) {
        static PBS: OnceLock<Mutex<HashMap<usize, ProgressBar>>> = OnceLock::new();
        static PB_TEMPLATE: LazyLock<ProgressStyle> = LazyLock::new(|| {
            ProgressStyle::default_bar()
                .template("[{bar:40}] {pos} / {len} {msg}")
                .unwrap()
                .progress_chars("=> ")
        });

        let mut pbs = PBS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap();
        let pb = pbs.entry(level_idx).or_insert_with(|| {
            let pb = ProgressBar::new(total_files as u64);
            pb.set_style(PB_TEMPLATE.clone());
            pb
        });

        pb.set_position(file as u64);
        pb.set_message(current_file_name);
    }

    let start = Instant::now();
    let bundle = AssetBundle::from_directory(&args.input_dir)?;
    println!("Files read in {}ms", start.elapsed().as_millis());

    let start = Instant::now();
    bundle.to_file(&args.output_bundle, args.compression_level, Some(cb))?;
    println!("Bundle created in {}ms", start.elapsed().as_millis());

    Ok(())
}
