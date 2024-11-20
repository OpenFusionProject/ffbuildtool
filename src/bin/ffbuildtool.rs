use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use clap::{Args, Parser, Subcommand};

use ffbuildtool::{Error, ItemProgress, Version};
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
    ExtractBundle(ExtractBundleArgs),
}

#[derive(Args, Debug)]
struct GenManifestArgs {
    /// Path to the directory containing all the compressed asset bundles in the build
    #[clap(short = 'b', long)]
    build_path: String,

    /// URL that will point to the directory containing all the compressed asset bundles in the build
    #[clap(short = 'u', long)]
    asset_url: String,

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
struct ExtractBundleArgs {
    /// Path to the compressed asset bundle
    #[clap(short = 'b', long)]
    bundle_path: String,

    /// Path to the output directory. Outputs will be stored under another directory named after the bundle
    #[clap(short = 'o', long)]
    output_dir: String,
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
                    .template("[{bar:40.cyan/blue}] {bytes} / {total_bytes} ({eta}) {wide_msg:>}")
                    .unwrap(),
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} Validating {wide_msg:>}")
                    .unwrap(),
            ],
        }
    }

    fn update_item(&self, name: &str, progress: ItemProgress) {
        match progress {
            ItemProgress::Downloading(current, total) => {
                self.update_item_downloading(name, current, total);
            }
            ItemProgress::Validating => {
                self.update_item_validating(name);
            }
            ItemProgress::Completed(_) | ItemProgress::Failed => {
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

static PROGRESS: OnceLock<ProgressManager> = OnceLock::new();
fn progress_callback(_uuid: &Uuid, name: &str, progress: ItemProgress) {
    PROGRESS.get().unwrap().update_item(name, progress);
}

#[tokio::main]
async fn main() -> Result<(), Error> {
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
        Commands::ExtractBundle(args) => extract_bundle(args).await,
    }
}

async fn generate_manifest(args: GenManifestArgs) -> Result<(), Error> {
    println!(
        "Generating manifest for build at {} with asset URL {}",
        args.build_path, args.asset_url
    );
    let parent_uuid: Option<Uuid> = if let Some(p) = args.parent {
        Some(Uuid::parse_str(p.as_str())?)
    } else {
        None
    };

    let mut version = Version::build(
        &args.build_path,
        &args.asset_url,
        args.description.as_deref(),
        parent_uuid,
    )
    .await?;

    if args.hidden {
        version.set_hidden(true);
    }

    println!("Build UUID: {}", version.get_uuid());

    version.export_manifest(&args.output_path)?;
    println!("Manifest exported to {}", args.output_path);
    Ok(())
}

async fn download_build(args: DownloadBuildArgs) -> Result<(), Error> {
    let version = Version::from_manifest(&args.manifest_path).await?;
    println!(
        "Downloading build {} to {}",
        version.get_uuid(),
        args.output_path
    );
    version
        .download_compressed(&args.output_path, Some(progress_callback))
        .await?;
    println!("Download complete");
    Ok(())
}

async fn repair_build(args: RepairBuildArgs) -> Result<(), Error> {
    let version = Version::from_manifest(&args.manifest_path).await?;
    println!(
        "Repairing build {} at {}",
        version.get_uuid(),
        args.build_path
    );
    let corrupted = version
        .repair(&args.build_path, Some(progress_callback))
        .await?;
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

async fn validate_build(args: ValidateBuildArgs) -> Result<(), Error> {
    let version = Version::from_manifest(&args.manifest_path).await?;
    println!(
        "Validating build {} at {}",
        version.get_uuid(),
        args.build_path
    );

    let corrupted = if args.uncompressed {
        version
            .validate_uncompressed(&args.build_path, None)
            .await?
    } else {
        version
            .validate_compressed(&args.build_path, Some(progress_callback))
            .await?
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
async fn extract_bundle(args: ExtractBundleArgs) -> Result<(), Error> {
    use ffbuildtool::bundle::AssetBundle;
    println!(
        "Extracting bundle {} to {}",
        args.bundle_path, args.output_dir
    );
    let asset_bundle = AssetBundle::from_file(&args.bundle_path)?;
    asset_bundle.extract_files(&args.output_dir)
}
