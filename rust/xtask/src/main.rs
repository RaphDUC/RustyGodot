//! # XTask Build System
//!
//! This is a custom build tool written in Rust effectively replacing Makefiles or Shell scripts.
//! It handles:
//! - Downloading Godot Engine.
//! - Downloading Export Templates.
//! - Building Rust crates (Server & Client).
//! - Generating GDExtension configuration.
//! - Packaging the game for release.
//!
//! Usage: `cargo xtask <command>`

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const GODOT_VERSION: &str = "4.6-stable";
const GODOT_VERSION_FULL: &str = "4.6.0-stable";

const BASE_URL: &str = "https://github.com/godotengine/godot/releases/download";

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Commands available in the build system.
#[derive(Subcommand)]
enum Commands {
    /// Download and setup Godot Engine and Templates
    Setup,
    /// Build Rust crates and copy artifacts to game/bin
    Build {
        #[arg(long)]
        release: bool,
    },
    /// Build and open the Godot Editor
    Editor,
    /// Build and run the game
    Run,
    /// Build and Package the game for distribution (creates an executable)
    Package
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // Instead of using current_dir directly, find the workspace root.
    // This allows running xtask from subdirectories (e.g. rust/).
    let root = find_project_root()?;

    match cli.command {
        Commands::Setup => setup_godot(&root)?,
        Commands::Build { release } => build_and_install(&root, release)?,
        Commands::Editor => {
            build_and_install(&root, false)?;
            run_godot(&root, true)?;
        }
        Commands::Run => {
            build_and_install(&root, false)?;
            run_godot(&root, false)?;
        },
        Commands::Package => {
            build_and_install(&root, true)?;
            ensure_export_presets(&root.join("game"))?;
            package_game(&root)?;
        }
    }

    Ok(())
}

fn find_project_root() -> Result<PathBuf> {
    let current_dir = std::env::current_dir()?;
    let mut path = current_dir.as_path();

    loop {
        // Unambiguous check: The root must contain "Cargo.lock" (workspace)
        // AND "game" (Godot project) AND "rust" (Source code).
        if path.join("Cargo.lock").exists()
            && path.join("game").exists()
            && path.join("rust").exists()
        {
            return Ok(path.to_path_buf());
        }

        match path.parent() {
            Some(p) => path = p,
            None => anyhow::bail!("Could not find project root! Are you running this inside the project folder?"),
        }
    }
}

fn get_os_info() -> (&'static str, &'static str) {
    if cfg!(target_os = "windows") {
        ("win64.exe.zip", "Godot_v4.6-stable_win64.exe")
    } else if cfg!(target_os = "macos") {
        ("macos.universal.zip", "Godot.app/Contents/MacOS/Godot")
    } else {
        ("linux.x86_64.zip", "Godot_v4.6-stable_linux.x86_64")
    }
}

fn setup_godot(root: &Path) -> Result<()> {
    let (zip_suffix, bin_relative_path) = get_os_info();
    let bin_dir = root.join(".godot_bin");
    
    if !bin_dir.exists() {
        fs::create_dir(&bin_dir)?;
    }

    // Download Editor
    let version_tag = GODOT_VERSION;
    let url = format!("{BASE_URL}/{version_tag}/Godot_v{version_tag}_{zip_suffix}");
    
    println!("Downloading Godot from: {}", url);
    let client = reqwest::blocking::Client::builder()
        .timeout(None) // Disable timeout completely for large files
        .build()?;

    let response = client.get(&url).send()?.bytes()?;
    
    println!("Extracting...");
    zip::ZipArchive::new(Cursor::new(response))?.extract(&bin_dir)?;

    let binary_path = bin_dir.join(bin_relative_path);
    if !binary_path.exists() {
        anyhow::bail!("Extracted binary not found at {:?}", binary_path);
    }

    // Fix Permissions (Linux & Mac)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(&binary_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary_path, perms)?;
        println!("Fixed permissions for: {:?}", binary_path);

        // MAC SPECIFIC: Remove the "Quarantine" attribute
        // macOS blocks downloaded binaries by default (Gatekeeper).
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("xattr")
                .arg("-d")
                .arg("com.apple.quarantine")
                .arg(&bin_dir.join("Godot.app"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    println!("Godot Setup Complete at {:?}", bin_dir);

    println!("Checking Export Templates...");

    // Determine Godot's standard template path
    let template_dir = get_godot_templates_dir()?;
    let version_dir = template_dir.join(GODOT_VERSION_FULL);

    if version_dir.exists() {
        println!("Templates already installed at {:?}", version_dir);
        return Ok(());
    }

    // Download the export templates
    let version_tag = GODOT_VERSION;
    let url = format!("{BASE_URL}/{version_tag}/Godot_v{version_tag}_export_templates.tpz");

    println!("Downloading Export Templates from: {}", url);
    let client = reqwest::blocking::Client::builder()
        .timeout(None) // Disable timeout completely for large files
        .build()?;

    let response = client.get(&url).send()?.bytes()?;

    println!("Extracting templates...");
    let mut archive = zip::ZipArchive::new(Cursor::new(response))?;

    // Extract to a temporary folder first
    let tmp_extract = root.join(".godot_bin/tmp_templates");
    if tmp_extract.exists() { fs::remove_dir_all(&tmp_extract)?; }
    archive.extract(&tmp_extract)?;

    // Move to System Folder
    fs::create_dir_all(&template_dir)?;
    // The zip extracts a folder called "templates". We move/rename it.
    let extracted_folder = tmp_extract.join("templates");
    if !extracted_folder.exists() {
        anyhow::bail!("Expected 'templates' folder in .tpz archive");
    }

    match fs::rename(&extracted_folder, &version_dir) {
        Ok(_) => {},
        Err(_) => {
            // Fallback: Copy if rename fails (cross-device link)
            let options = fs_extra::dir::CopyOptions::new().content_only(true);
            fs::create_dir_all(&version_dir)?;
            fs_extra::dir::copy(&extracted_folder, &version_dir, &options)?;
        }
    }

    // Cleanup
    fs::remove_dir_all(&tmp_extract)?;

    println!("Export Templates installed to {:?}", version_dir);
    Ok(())
}

fn generate_gdextension_file(game_dir: &Path, crate_name: &str) -> Result<()> {
    let gdext_path = game_dir.join(format!("{}.gdextension", crate_name));

    let content = format!(r#"
[configuration]
entry_symbol = "gdext_rust_init"
compatibility_minimum = "4.1"

[libraries]
linux.debug.x86_64 = "res://bin/{crate_name}/linux/lib{crate_name}.so"
linux.release.x86_64 = "res://bin/{crate_name}/linux/lib{crate_name}.so"
macos.debug.arm64 = "res://bin/{crate_name}/macos/arm64/lib{crate_name}.dylib"
macos.release.arm64 = "res://bin/{crate_name}/macos/arm64/lib{crate_name}.dylib"
windows.debug.x86_64 = "res://bin/{crate_name}/windows/{crate_name}.dll"
windows.release.x86_64 = "res://bin/{crate_name}/windows/{crate_name}.dll"
"#);

    fs::write(&gdext_path, content.trim())?;
    println!("Generated .gdextension file at: {:?}", gdext_path);

    Ok(())
}

fn build_and_install(root: &Path, release: bool) -> Result<()> {
    println!("Building Rust crates (Game only)...");

    let mut cmd = Command::new("cargo");
    cmd.current_dir(root);
    cmd.arg("build");
    cmd.arg("-p").arg("game");
    if release {
        cmd.arg("--release");
    }
    
    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("Cargo build failed");
    }

    // Move Artifacts
    let target_dir = root.join("target").join(if release { "release" } else { "debug" });
    let output_dir = root.join("game/bin/game");
    
    if !output_dir.exists() {
        fs::create_dir_all(&output_dir)?;
    }

    let (ext, crate_name, output_dir ) = if cfg!(target_os = "windows") {
        ("dll", "game", output_dir.join("windows"))
    } else if cfg!(target_os = "linux") {
        ("so", "libgame", output_dir.join("linux"))
    } else if cfg!(target_os = "macos") {
        ("dylib", "libgame", output_dir.join("macos").join("arm64"))
    } else {
        anyhow::bail!("Unsupported OS");
    };

    if !output_dir.exists(){
        fs::create_dir_all(&output_dir)?;
    }

    // Copy specific crate output
    let src = target_dir.join(format!("{}.{}", crate_name, ext));
    let dst = output_dir.join(format!("{}.{}", crate_name, ext));

    if src.exists() {
        fs::copy(&src, &dst)?;
        println!("Copied artifact to {:?}", dst);
        // Generate the configuration
        let game_dir = root.join("game");
        generate_gdextension_file(&game_dir, "game")?;
    } else {
        anyhow::bail!("Failed to find artifact: {:?}", src);
    }

    Ok(())
}

fn run_godot(root: &Path, editor: bool) -> Result<()> {
    let (_, bin_relative_path) = get_os_info();
    let bin_dir = root.join(".godot_bin");
    let godot_exe = bin_dir.join(bin_relative_path);

    if !godot_exe.exists() {
        anyhow::bail!("Godot executable not found. Run 'cargo xtask setup' first.");
    }

    let game_dir = root.join("game");
    if !game_dir.exists() {
        fs::create_dir_all(&game_dir)?;
    }

    // Auto-Create 'project.godot' if missing
    // This prevents the "Project Manager" wizard from appearing and complaining.
    let project_file = game_dir.join("project.godot");
    if !project_file.exists() {
        println!("project.godot missing. Creating minimal project...");

        // Minimal Godot 4.6 config
        let content = r#"; Engine configuration file.
config_version=5

[application]
config/name="My Rust Game"
config/features=PackedStringArray("4.6", "Forward Plus")
config/icon="res://icon.svg"

[dotnet]
project/assembly_name="My Rust Game"
"#;
        let mut file = fs::File::create(&project_file)?;
        file.write_all(content.as_bytes())?;
    }

    // Get Absolute Paths
    let godot_exe_abs = godot_exe.canonicalize()
        .context("Failed to canonicalize Godot executable path")?;

    let game_dir_abs = game_dir.canonicalize()?;

    let mut cmd = Command::new(&godot_exe_abs);

    // This makes Godot treat 'game/' as the root, avoiding the "working directory" error.
    cmd.current_dir(&game_dir_abs);

    if editor {
        cmd.arg("-e");
    }

    cmd.arg("--path").arg(&game_dir_abs);

    println!("Launching Godot...");
    cmd.status().context("Failed to launch Godot process")?;

    Ok(())
}

fn ensure_export_presets(game_dir: &Path) -> Result<()> {
    let presets_path = game_dir.join("export_presets.cfg");
    if presets_path.exists() {
        return Ok(());
    }

    println!("Generating export_presets.cfg...");

    // Generate a preset for the current OS so 'package' works out of the box.
    let (platform_name, _) = get_platform_export_name();

    let content = format!(r#"
[preset.0]

name="{platform_name}"
platform="{platform_name}"
runnable=true
custom_features=""
export_filter="all_resources"
include_filter=""
exclude_filter=""
export_path="../builds/{platform_name}/game"
patch_list=PackedStringArray()
"#);

    fs::write(&presets_path, content.trim())?;
    Ok(())
}

fn package_game(root: &Path) -> Result<()> {
    let (_, bin_relative_path) = get_os_info();
    let godot_exe = root.join(".godot_bin").join(bin_relative_path);
    let game_dir = root.join("game");

    // Ensure build output directory exists
    let builds_dir = root.join("builds");
    if !builds_dir.exists() {
        fs::create_dir(&builds_dir)?;
    }

    let (platform_name, output_ext) = get_platform_export_name();
    let output_path = builds_dir.join(platform_name).join(format!("game{}", output_ext));

    // Create the specific platform folder (e.g., builds/Linux)
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    println!("Exporting project for {}...", platform_name);

    let godot_abs = godot_exe.canonicalize()?;
    let game_abs = game_dir.canonicalize()?;
    let output_abs = output_path; // Don't canonicalize yet, might not exist

    println!("Step 1/2: Importing assets for {}...", platform_name);
    let status_import = Command::new(&godot_abs)
        .arg("--headless")
        .arg("--editor")
        .arg("--quit") // Quit immediately after import
        .arg("--audio-driver").arg("Dummy")
        .arg("--display-driver").arg("headless")
        .current_dir(&game_abs)
        .status()?;

    if !status_import.success() {
        anyhow::bail!("Godot Import step failed.");
    }

    println!("Step 2/2: Exporting project for {}...", platform_name);
    let status_export = Command::new(&godot_abs)
        .arg("--headless")
        .arg("--audio-driver").arg("Dummy")
        .arg("--display-driver").arg("headless")
        .arg("--export-release")
        .arg(platform_name)
        .arg(output_abs)
        .current_dir(&game_abs)
        .status()?;

    if status_export.success() {
        println!("Export complete! Find it at: builds/{}/", platform_name);
    } else {
        anyhow::bail!("Godot Export step failed.");
    }

    Ok(())
}

fn get_platform_export_name() -> (&'static str, &'static str) {
    if cfg!(target_os = "windows") {
        ("Windows Desktop", ".exe")
    } else if cfg!(target_os = "macos") {
        ("macOS", ".zip")
    } else {
        ("Linux", "")
    }
}

fn get_godot_templates_dir() -> Result<PathBuf> {
    // Standard Godot paths:
    // Linux: ~/.local/share/godot/export_templates/
    // macOS: ~/Library/Application Support/Godot/export_templates/
    // Windows: %APPDATA%\Godot\export_templates\

    let path = if cfg!(target_os = "macos") {
        dirs::home_dir().unwrap().join("Library/Application Support/Godot/export_templates")
    } else if cfg!(target_os = "windows") {
        dirs::data_dir().unwrap().join("Godot/export_templates")
    } else {
        dirs::data_local_dir().unwrap().join("godot/export_templates")
    };

    Ok(path)
}
