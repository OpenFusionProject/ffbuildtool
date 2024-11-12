use clap::{Args, Parser, Subcommand};

use ffbuildtool::{Error, Version};
use log::LevelFilter;
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

#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::builder()
        .format_timestamp(None)
        .filter_level(LevelFilter::Info)
        .init();

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
    let parent_uuid: Option<Uuid> = if let Some(p) = args.parent {
        Some(Uuid::parse_str(p.as_str())?)
    } else {
        None
    };

    let version = Version::build(
        &args.build_path,
        &args.asset_url,
        args.description.as_deref(),
        parent_uuid,
    )
    .await?;

    version.export_manifest(&args.output_path)
}

async fn download_build(args: DownloadBuildArgs) -> Result<(), Error> {
    let version = Version::from_manifest(&args.manifest_path).await?;
    version.download_compressed(&args.output_path).await
}

async fn repair_build(args: RepairBuildArgs) -> Result<(), Error> {
    let version = Version::from_manifest(&args.manifest_path).await?;
    let corrupted = version.repair(&args.build_path).await?;
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
    let corrupted = if args.uncompressed {
        version.validate_uncompressed(&args.build_path).await?
    } else {
        version.validate_compressed(&args.build_path).await?
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

    let asset_bundle = AssetBundle::from_file(&args.bundle_path)?;
    asset_bundle.extract_files(&args.output_dir)
}
